import { test } from "vitest";
import assert from "node:assert/strict";
import { publishRelease } from "./publish-release.mjs";
for (const failAt of ["createDraft", "upload", "verify"]) {
  test("interruption during " + failAt + " preserves previous updater target", async () => {
    let latest = "previous";
    const calls = [];
    const api = Object.fromEntries(["createDraft", "upload", "verify", "activate"].map(name => [name, async () => {
      calls.push(name);
      if (name === failAt) throw new Error("injected interruption");
      if (name === "activate") latest = "candidate";
      return { id: 17, draft: true, prerelease: false };
    }]));
    await assert.rejects(publishRelease(api, { prerelease: false }));
    assert.equal(latest, "previous");
    assert.equal(calls.includes("activate"), false);
  });
}
test("activation follows verification of all staged assets", async () => {
  const calls = [];
  await publishRelease({
    createDraft: async () => ({ id: 17, draft: true, prerelease: false }),
    upload: async () => calls.push("upload"),
    verify: async () => calls.push("verify"),
    activate: async (id, prerelease) => { assert.equal(id, 17); assert.equal(prerelease, false); calls.push("activate"); },
  }, { prerelease: false });
  assert.deepEqual(calls, ["upload", "verify", "activate"]);
});
test("refuses a public or wrong-channel staging response", async () => {
  for (const draft of [{id:17,draft:false,prerelease:false}, {id:17,draft:true,prerelease:true}]) {
    await assert.rejects(publishRelease({createDraft:async()=>draft}, {prerelease:false}));
  }
});
test("RC activation remains a prerelease", async () => {
  await publishRelease({
    createDraft: async () => ({id:17,draft:true,prerelease:true}),
    upload:async()=>{}, verify:async()=>{},
    activate:async(_, prerelease)=>assert.equal(prerelease,true),
  }, {prerelease:true});
});
