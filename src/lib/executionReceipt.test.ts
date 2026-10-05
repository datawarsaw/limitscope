import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  EXECUTION_RECEIPT_DISCLAIMER,
  EXECUTION_RECEIPT_KIND,
  EXECUTION_RECEIPT_SCHEMA_VERSION,
  createExecutionReceipt,
  exportExecutionReceipt,
  formatReceiptJson,
  formatReceiptMarkdown,
  receiptFileName,
} from "./executionReceipt";
import { buildProvenance, type ExecutionProvenanceRun, type ExecutionQuotaSnapshot } from "./provenance";

const STARTED = "2026-09-30T09:00:00Z";
const ENDED = "2026-09-30T09:18:00Z";
const ENDED_MS = Date.parse(ENDED);
const RESET_5H = "2026-09-30T14:00:00Z";
const RESET_WEEK = "2026-10-01T14:00:00Z";
const RUN_ID = "run-2026-09-30T0900Z-0001";

/** Assembled at runtime so this source file never contains a PAT shape. */
const FAKE_GITHUB_PAT = `ghp_${"a".repeat(26)}`;

const REPO_ROOT = fileURLToPath(new URL("../../", import.meta.url));
const CLI_PATH = fileURLToPath(new URL("../../scripts/execution-receipt.mjs", import.meta.url));

function snapshot(overrides: Partial<ExecutionQuotaSnapshot> = {}): ExecutionQuotaSnapshot {
  return {
    capturedAt: STARTED,
    providerId: "openai-codex",
    status: "ok",
    windows: [
      { label: "5-hour", usedPercent: 23, resetAt: RESET_5H },
      { label: "Weekly", usedPercent: 38, resetAt: RESET_WEEK },
    ],
    ...overrides,
  };
}

/** Mirrors the documented example: 23% -> 28% and 38% -> 40% over 18 minutes. */
function exampleRun(extra: Record<string, unknown> = {}): ExecutionProvenanceRun {
  return buildProvenance({
    before: snapshot(),
    after: snapshot({
      capturedAt: ENDED,
      windows: [
        { label: "5-hour", usedPercent: 28, resetAt: RESET_5H },
        { label: "Weekly", usedPercent: 40, resetAt: RESET_WEEK },
      ],
    }),
    harness: "Codex",
    runId: RUN_ID,
    startedAt: STARTED,
    endedAt: ENDED,
    nowMs: ENDED_MS,
    ...extra,
  });
}

function exampleRunWithMetadata(): ExecutionProvenanceRun {
  return buildProvenance({
    before: snapshot({ accountIdentity: "key:3456", planType: "team" }),
    after: snapshot({
      capturedAt: ENDED,
      accountIdentity: "key:3456",
      planType: "team",
      windows: [
        { label: "5-hour", usedPercent: 28, resetAt: RESET_5H },
        { label: "Weekly", usedPercent: 40, resetAt: RESET_WEEK },
      ],
    }),
    harness: "Codex",
    runId: RUN_ID,
    model: "GPT-6 Sol",
    reasoningEffort: "high",
    subscription: "team",
    startedAt: STARTED,
    endedAt: ENDED,
    nowMs: ENDED_MS,
  });
}

function resetCrossedRun(): ExecutionProvenanceRun {
  return buildProvenance({
    before: snapshot({
      windows: [
        { label: "5-hour", usedPercent: 23, resetAt: RESET_5H },
        { label: "Weekly", usedPercent: 38, resetAt: "2026-09-30T09:10:00Z" },
      ],
    }),
    after: snapshot({
      capturedAt: ENDED,
      windows: [
        { label: "5-hour", usedPercent: 28, resetAt: RESET_5H },
        { label: "Weekly", usedPercent: 3, resetAt: RESET_WEEK },
      ],
    }),
    harness: "Codex",
    runId: RUN_ID,
    startedAt: STARTED,
    endedAt: ENDED,
    nowMs: ENDED_MS,
  });
}

/** Raw run-shaped input (as read back from a run JSON file). */
function rawRun(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    runId: RUN_ID,
    startedAt: STARTED,
    endedAt: ENDED,
    harness: "Codex",
    providerId: "openai-codex",
    confidence: "BOUNDED",
    resetCrossed: false,
    comparable: true,
    windows: [
      { label: "5-hour", beforeUsedPercent: 23, afterUsedPercent: 28, deltaPoints: 5, comparable: true },
      { label: "Weekly", beforeUsedPercent: 38, afterUsedPercent: 40, deltaPoints: 2, comparable: true },
    ],
    ...overrides,
  };
}

function receiptOf(raw: unknown) {
  const receipt = createExecutionReceipt(raw);
  expect(receipt).toBeDefined();
  return receipt!;
}

const EXPECTED_EXAMPLE_MARKDOWN = [
  "LimitScope Execution Receipt",
  "",
  "Harness: Codex",
  "Duration: 18m",
  "Confidence: BOUNDED",
  "",
  "5-hour",
  "23% → 28%",
  "Observed delta: +5 pp",
  "",
  "Weekly",
  "38% → 40%",
  "Observed delta: +2 pp",
  "",
  "Observed quota delta during execution.",
  "Not exact task cost.",
  "",
].join("\n");

describe("execution receipt: JSON", () => {
  it("emits the versioned schema with production fields and no raw snapshots", () => {
    const json = formatReceiptJson(receiptOf(exampleRun()));
    const parsed = JSON.parse(json) as Record<string, unknown>;

    expect(parsed.schemaVersion).toBe(EXECUTION_RECEIPT_SCHEMA_VERSION);
    expect(parsed.kind).toBe(EXECUTION_RECEIPT_KIND);
    expect(parsed.runId).toBe(RUN_ID);
    expect(parsed.startedAt).toBe("2026-09-30T09:00:00.000Z");
    expect(parsed.endedAt).toBe("2026-09-30T09:18:00.000Z");
    expect(parsed.durationMs).toBe(18 * 60 * 1000);
    expect(parsed.harness).toBe("Codex");
    expect(parsed.providerId).toBe("openai-codex");
    expect(parsed.confidence).toBe("BOUNDED");
    expect(parsed.resetCrossed).toBe(false);
    expect(parsed.comparable).toBe(true);
    expect(parsed.disclaimer).toBe(EXECUTION_RECEIPT_DISCLAIMER);
    expect(parsed.windows).toEqual([
      {
        label: "5-hour",
        beforeUsedPercent: 23,
        afterUsedPercent: 28,
        deltaPoints: 5,
        comparable: true,
        status: "measured",
      },
      {
        label: "Weekly",
        beforeUsedPercent: 38,
        afterUsedPercent: 40,
        deltaPoints: 2,
        comparable: true,
        status: "measured",
      },
    ]);

    // Raw snapshots are not duplicated into the receipt.
    expect(json).not.toContain("beforeSnapshot");
    expect(json).not.toContain("afterSnapshot");
    expect(json).not.toContain("resetAt");
    expect(json).not.toContain("undefined");
    expect(json.endsWith("\n")).toBe(true);
    expect(json).not.toContain("\r");
  });

  it("keeps multiple windows sorted by label", () => {
    const parsed = JSON.parse(formatReceiptJson(receiptOf(rawRun()))) as { windows: { label: string }[] };
    expect(parsed.windows.map((w) => w.label)).toEqual(["5-hour", "Weekly"]);

    const reversed = rawRun({
      windows: [
        { label: "Weekly", beforeUsedPercent: 38, afterUsedPercent: 40, deltaPoints: 2, comparable: true },
        { label: "5-hour", beforeUsedPercent: 23, afterUsedPercent: 28, deltaPoints: 5, comparable: true },
      ],
    });
    const sorted = JSON.parse(formatReceiptJson(receiptOf(reversed))) as { windows: { label: string }[] };
    expect(sorted.windows.map((w) => w.label)).toEqual(["5-hour", "Weekly"]);
  });
});

describe("execution receipt: Markdown", () => {
  it("renders the compact documented shape", () => {
    expect(formatReceiptMarkdown(receiptOf(exampleRun()))).toBe(EXPECTED_EXAMPLE_MARKDOWN);
  });

  it("includes optional metadata only when present", () => {
    const markdown = formatReceiptMarkdown(receiptOf(exampleRunWithMetadata()));
    expect(markdown).toContain("Model: GPT-6 Sol");
    expect(markdown).toContain("Reasoning: high");
    expect(markdown).toContain("Plan: team");
    expect(markdown).toContain("Account: key:3456");
    const parsed = JSON.parse(formatReceiptJson(receiptOf(exampleRunWithMetadata()))) as Record<string, unknown>;
    expect(parsed.model).toBe("GPT-6 Sol");
    expect(parsed.reasoningEffort).toBe("high");
    expect(parsed.subscription).toBe("team");
    expect(parsed.account).toBe("key:3456");
  });

  it("omits missing model and reasoning instead of rendering placeholders", () => {
    const markdown = formatReceiptMarkdown(receiptOf(exampleRun()));
    expect(markdown).not.toContain("Model:");
    expect(markdown).not.toContain("Reasoning:");
    const parsed = JSON.parse(formatReceiptJson(receiptOf(exampleRun()))) as Record<string, unknown>;
    expect(parsed).not.toHaveProperty("model");
    expect(parsed).not.toHaveProperty("reasoningEffort");
    expect(markdown).not.toMatch(/undefined|null|unknown/i);
  });

  it("never infers a plan from the model name", () => {
    const markdown = formatReceiptMarkdown(receiptOf(exampleRun({ model: "GPT-6 Pro" })));
    expect(markdown).toContain("Model: GPT-6 Pro");
    expect(markdown).not.toContain("Plan:");
  });

  it("omits account attribution when unavailable", () => {
    const markdown = formatReceiptMarkdown(receiptOf(exampleRun()));
    expect(markdown).not.toContain("Account:");
    const parsed = JSON.parse(formatReceiptJson(receiptOf(exampleRun()))) as Record<string, unknown>;
    expect(parsed).not.toHaveProperty("account");
  });

  it("carries the BOUNDED observation disclaimer without claiming task cost", () => {
    const markdown = formatReceiptMarkdown(receiptOf(exampleRun()));
    expect(markdown).toContain("Confidence: BOUNDED");
    expect(markdown).toContain("Observed quota delta during execution.");
    expect(markdown).toContain("Not exact task cost.");
    expect(markdown).not.toMatch(/task cost:/i);
    expect(markdown).not.toMatch(/cost of (this|the) task/i);
  });

  it("reports INFERRED confidence", () => {
    const stale = buildProvenance({
      before: snapshot({ capturedAt: "2026-09-29T08:00:00Z" }),
      after: snapshot({
        capturedAt: ENDED,
        windows: [
          { label: "5-hour", usedPercent: 28, resetAt: RESET_5H },
          { label: "Weekly", usedPercent: 40, resetAt: RESET_WEEK },
        ],
      }),
      harness: "Codex",
      runId: RUN_ID,
      startedAt: STARTED,
      endedAt: ENDED,
      nowMs: ENDED_MS,
    });
    expect(stale.confidence).toBe("INFERRED");
    const markdown = formatReceiptMarkdown(receiptOf(stale));
    expect(markdown).toContain("Confidence: INFERRED");
    expect(markdown).toContain("Observed delta: +5 pp");
  });

  it("renders an UNAVAILABLE run without window deltas", () => {
    const unavailable = buildProvenance({
      before: snapshot({ windows: [] }),
      after: snapshot({ capturedAt: ENDED, windows: [] }),
      harness: "Codex",
      runId: RUN_ID,
      startedAt: STARTED,
      endedAt: ENDED,
      nowMs: ENDED_MS,
    });
    expect(unavailable.confidence).toBe("UNAVAILABLE");

    const markdown = formatReceiptMarkdown(receiptOf(unavailable));
    expect(markdown).toContain("Confidence: UNAVAILABLE");
    expect(markdown).toContain("Quota observation unavailable");
    expect(markdown).not.toContain("Observed delta:");
    expect(markdown).toContain("Not exact task cost.");

    const parsed = JSON.parse(formatReceiptJson(receiptOf(unavailable))) as Record<string, unknown>;
    expect(parsed.confidence).toBe("UNAVAILABLE");
    expect(parsed.comparable).toBe(false);
    expect(parsed.windows).toEqual([]);
  });

  it("drops to UNAVAILABLE when a partial snapshot yields no comparable window", () => {
    const partial = buildProvenance({
      before: snapshot({ windows: [] }),
      after: snapshot({
        capturedAt: ENDED,
        windows: [{ label: "5-hour", usedPercent: 28, resetAt: RESET_5H }],
      }),
      harness: "Codex",
      runId: RUN_ID,
      startedAt: STARTED,
      endedAt: ENDED,
      nowMs: ENDED_MS,
    });
    expect(partial.confidence).toBe("UNAVAILABLE");
    const markdown = formatReceiptMarkdown(receiptOf(partial));
    expect(markdown).toContain("Comparison unavailable");
    expect(markdown).not.toContain("Observed delta:");
  });
});

describe("execution receipt: reset crossing", () => {
  it("never exports the misleading arithmetic delta", () => {
    const run = resetCrossedRun();
    expect(run.resetCrossed).toBe(true);

    const markdown = formatReceiptMarkdown(receiptOf(run));
    expect(markdown).toContain("Weekly\nReset occurred during execution\nDelta unavailable");
    expect(markdown).toContain("Quota reset occurred during execution.");
    expect(markdown).toContain("Not exact task cost.");
    expect(markdown).not.toContain("Observed delta:");
    expect(markdown).not.toContain("-35");

    const parsed = JSON.parse(formatReceiptJson(receiptOf(run))) as {
      resetCrossed: boolean;
      comparable: boolean;
      windows: { label: string; deltaPoints: number | null; comparable: boolean; status: string }[];
    };
    expect(parsed.resetCrossed).toBe(true);
    expect(parsed.comparable).toBe(false);
    const weekly = parsed.windows.find((w) => w.label === "Weekly");
    expect(weekly).toBeDefined();
    expect(weekly!.deltaPoints).toBeNull();
    expect(weekly!.comparable).toBe(false);
    expect(weekly!.status).toBe("reset-crossed");
    for (const window of parsed.windows) expect(window.deltaPoints).toBeNull();
  });
});

describe("execution receipt: privacy", () => {
  it("drops secret-shaped identity and metadata fields", () => {
    const leaked = rawRun({
      runId: FAKE_GITHUB_PAT,
      providerId: "sk-provider-key",
      model: "sk-proj-1234567890abcdef",
      reasoningEffort: "Bearer abcdef",
      account: "eyJhbGciOiJIUzI1NiJ9.payload.signature",
      planType: "token-plan",
    });
    const receipt = receiptOf(leaked);
    const json = formatReceiptJson(receipt);
    const markdown = formatReceiptMarkdown(receipt);

    for (const text of [json, markdown]) {
      expect(text).not.toMatch(/eyj|sk-|ghp_|bearer|token|secret|@/i);
    }
    expect(json).not.toMatch(/"(account|model|reasoningEffort|runId|providerId|subscription)"/);
    expect(markdown).not.toContain("Account:");
    expect(markdown).not.toContain("Plan:");

    // Already-masked attribution still passes through untouched.
    const masked = receiptOf(rawRun({ account: "key:3456", planType: "team" }));
    expect(masked.account).toBe("key:3456");
    expect(masked.subscription).toBe("team");
    expect(formatReceiptMarkdown(masked)).toContain("Account: key:3456");
  });
});

describe("execution receipt: special characters", () => {
  it("neutralizes markup and line breaks so no value can inject a block", () => {
    const noisy = rawRun({
      model: "GPT\n6\tTurbo",
      windows: [
        {
          label: 'Weekly "7-day" <script>alert(1)</script>&more',
          beforeUsedPercent: 38,
          afterUsedPercent: 40,
          deltaPoints: 2,
          comparable: true,
        },
      ],
    });
    const receipt = receiptOf(noisy);
    const markdown = formatReceiptMarkdown(receipt);

    expect(markdown).toContain('Weekly "7-day" &lt;script&gt;alert(1)&lt;/script&gt;&amp;more');
    expect(markdown).toContain("Model: GPT 6 Turbo");
    expect(markdown).toContain("38% → 40%");
    expect(markdown).not.toContain("<script>");
    expect(markdown).not.toContain("\r");
    expect(markdown.split("\n").filter((line) => line.startsWith('Weekly "7-day"'))).toHaveLength(1);

    const parsed = JSON.parse(formatReceiptJson(receipt)) as { windows: { label: string }[]; model?: string };
    expect(parsed.windows[0].label).toBe('Weekly "7-day" <script>alert(1)</script>&more');
    expect(parsed.model).toBe("GPT 6 Turbo");
  });
});

describe("execution receipt: determinism", () => {
  it("produces byte-identical output for repeated and reordered input", () => {
    const forward = rawRun();
    const reordered = rawRun({
      windows: [
        { label: "Weekly", beforeUsedPercent: 38, afterUsedPercent: 40, deltaPoints: 2, comparable: true },
        { label: "5-hour", beforeUsedPercent: 23, afterUsedPercent: 28, deltaPoints: 5, comparable: true },
      ],
    });

    const first = formatReceiptMarkdown(receiptOf(forward));
    const second = formatReceiptMarkdown(receiptOf(forward));
    const shuffled = formatReceiptMarkdown(receiptOf(reordered));
    expect(first).toBe(second);
    expect(first).toBe(shuffled);

    const firstJson = formatReceiptJson(receiptOf(forward));
    expect(firstJson).toBe(formatReceiptJson(receiptOf(forward)));
    expect(firstJson).toBe(formatReceiptJson(receiptOf(reordered)));

    expect(first.endsWith("\n")).toBe(true);
    expect(first.endsWith("\n\n")).toBe(false);
    expect(first).not.toContain("\r");
    expect(firstJson.endsWith("\n")).toBe(true);
  });

  it("names files deterministically without account identifiers", () => {
    const receipt = receiptOf(exampleRunWithMetadata());
    expect(receiptFileName(receipt, "markdown")).toBe("limitscope-run-2026-09-30T0900Z.md");
    expect(receiptFileName(receipt, "json")).toBe("limitscope-run-2026-09-30T0900Z.json");
    expect(receiptFileName(receipt, "markdown")).not.toContain("key:3456");

    const exported = exportExecutionReceipt(exampleRunWithMetadata(), "json");
    expect(exported?.fileName).toBe("limitscope-run-2026-09-30T0900Z.json");
    expect(exported?.content).toBe(formatReceiptJson(receipt));
    expect(exportExecutionReceipt({}, "json")).toBeUndefined();
  });
});

describe("execution receipt: CLI fixture exporter", () => {
  function runCli(args: string[]): string {
    return execFileSync(process.execPath, [CLI_PATH, ...args], { cwd: REPO_ROOT, encoding: "utf8" });
  }

  it("matches the library output byte-for-byte and writes deterministic file names", () => {
    const dir = mkdtempSync(`${tmpdir()}/limitscope-receipt-`);
    try {
      const fixtures: { run: ExecutionProvenanceRun; name: string }[] = [
        { run: exampleRunWithMetadata(), name: "metadata" },
        { run: resetCrossedRun(), name: "reset" },
      ];

      for (const { run, name } of fixtures) {
        const inputPath = `${dir}/${name}.run.json`;
        writeFileSync(inputPath, `${JSON.stringify(run, null, 2)}\n`, "utf8");
        const expected = createExecutionReceipt(run)!;

        const json = runCli(["--input", inputPath, "--format", "json", "--stdout"]);
        const markdown = runCli(["--input", inputPath, "--format", "markdown", "--stdout"]);
        expect(json).toBe(formatReceiptJson(expected));
        expect(markdown).toBe(formatReceiptMarkdown(expected));
        expect(runCli(["--input", inputPath, "--format", "markdown", "--stdout"])).toBe(markdown);

        runCli(["--input", inputPath, "--format", "markdown", "--out-dir", dir]);
        const written = readFileSync(`${dir}/${receiptFileName(expected, "markdown")}`, "utf8");
        expect(written).toBe(markdown);
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
