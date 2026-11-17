import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

// Failed drafts remain inspectable. A retry never edits a public release.
export async function publishRelease(api, release) {
  const draft = await api.createDraft(release);
  if (!draft.draft || draft.prerelease !== release.prerelease || !Number.isInteger(draft.id)) {
    throw new Error("Release staging did not return the expected draft");
  }
  await api.upload(release);
  await api.verify(release, draft.id);
  await api.activate(draft.id, release.prerelease);
}

function productionApi(env) {
  const repo = env.PUBLIC_FEED_REPO;
  const tag = "v" + env.VERSION;
  const files = [env.INSTALLER_PATH, env.SIG_PATH, "latest.json"];
  const expected = new Map(files.map(file => [basename(file), readFileSync(file)]));
  if (expected.size !== 3) throw new Error("Expected exactly three distinct release artifacts");
  const digest = bytes => createHash("sha256").update(bytes).digest("hex");
  const manifest = readFileSync("release-artifacts-manifest.txt", "utf8");
  if (!/^[0-9a-f]{40}$/.test(env.GITHUB_SHA ?? "") || !manifest.includes("source_sha: " + env.GITHUB_SHA) ||
      digest(expected.get(basename(files[0]))) !== env.INSTALLER_SHA256 ||
      digest(expected.get(basename(files[1]))) !== env.SIG_SHA256) {
    throw new Error("Release artifacts do not match the verified build provenance");
  }
  const feed = JSON.parse(expected.get("latest.json"));
  const platform = feed.platforms?.["windows-x86_64"];
  const url = "https://github.com/" + repo + "/releases/download/" + tag + "/" + encodeURIComponent(basename(files[0]));
  if (feed.version !== env.VERSION || platform?.url !== url ||
      platform?.signature?.trim() !== readFileSync(files[1], "utf8").trim()) {
    throw new Error("Installer, signature and feed are inconsistent");
  }
  const gh = (args, input) => execFileSync("gh", args, { encoding: "utf8", input, stdio: ["pipe", "pipe", "pipe"] });
  const endpoint = "repos/" + repo + "/releases";
  return {
    createDraft: async ({ prerelease }) => {
      return JSON.parse(gh(["api", "-X", "POST", endpoint, "--input", "-"], JSON.stringify({
        tag_name: tag, target_commitish: "main", draft: true, prerelease,
        make_latest: "false", name: "LimitScope " + env.VERSION, body: env.RELEASE_NOTES ?? "",
      })));
    },
    upload: async () => gh(["release", "upload", tag, "--repo", repo, ...files]),
    verify: async (_, id) => {
      const release = JSON.parse(gh(["api", endpoint + "/" + id]));
      if (!release.draft || release.tag_name !== tag ||
          release.assets.length !== expected.size ||
          release.assets.some(asset => !expected.has(asset.name) || asset.size !== expected.get(asset.name).length)) {
        throw new Error("Draft identity or asset inventory changed");
      }
      const dir = mkdtempSync(join(tmpdir(), "limitscope-release-verification-"));
      try {
        gh(["release", "download", tag, "--repo", repo, "--dir", dir]);
        for (const [name, bytes] of expected) {
          if (!readFileSync(join(dir, name)).equals(bytes)) throw new Error("Uploaded asset bytes differ");
        }
      } finally { rmSync(dir, { recursive: true, force: true }); }
    },
    activate: async (id, prerelease) => gh(["api", "-X", "PATCH", endpoint + "/" + id, "--input", "-"],
      JSON.stringify({ draft: false, prerelease, make_latest: prerelease ? "false" : "true" })),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const env = process.env;
  if (!/^\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(env.VERSION ?? "") ||
      !["stable", "rc"].includes(env.RELEASE_CHANNEL)) throw new Error("Invalid release identity");
  if ((env.RELEASE_CHANNEL === "rc") !== env.VERSION.includes("-rc.")) throw new Error("Channel mismatch");
  await publishRelease(productionApi(env), { prerelease: env.RELEASE_CHANNEL === "rc" });
}
