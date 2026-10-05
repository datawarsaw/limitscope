import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");

function cssSyntaxErrors(source: string): string[] {
  const errors: string[] = [];
  let depth = 0;
  let quote: string | null = null;
  let escaped = false;
  let comment = false;
  let line = 1;
  for (let i = 0; i < source.length; i += 1) {
    const ch = source[i];
    const next = source[i + 1];
    if (ch === "\n") line += 1;
    if (comment) {
      if (ch === "*" && next === "/") {
        comment = false;
        i += 1;
      }
      continue;
    }
    if (quote !== null) {
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === "/" && next === "*") {
      comment = true;
      i += 1;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === "{") {
      depth += 1;
    } else if (ch === "}") {
      depth -= 1;
      if (depth < 0) errors.push(`extra closing brace at line ${line}`);
    }
  }
  if (comment) errors.push("unclosed CSS comment");
  if (quote !== null) errors.push("unclosed CSS string");
  if (depth !== 0) errors.push(`unbalanced CSS braces (depth ${depth})`);
  return errors;
}

describe("styles.css syntax guard", () => {
  it("keeps every CSS block syntactically complete and brace-balanced", () => {
    expect(cssSyntaxErrors(css)).toEqual([]);
  });

  it("does not let a splice leave declarations outside a rule block", () => {
    expect(css).toContain(".local-data-row .setting-success,");
    expect(css).toContain(".local-data-confirm-btn:focus-visible");
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
  });
});
