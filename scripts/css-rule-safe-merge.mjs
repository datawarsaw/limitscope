#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";

function findMatchingBrace(source, openAt) {
  let depth = 0;
  let quote = null;
  let escaped = false;
  let comment = false;
  for (let i = openAt; i < source.length; i += 1) {
    const ch = source[i];
    const next = source[i + 1];
    if (comment) {
      if (ch === "*" && next === "/") {
        comment = false;
        i += 1;
      }
      continue;
    }
    if (quote !== null) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === quote) {
        quote = null;
      }
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
      if (depth === 0) return i;
      if (depth < 0) throw new Error(`extra closing brace at offset ${i}`);
    }
  }
  throw new Error(`unclosed block starting at offset ${openAt}`);
}

export function parseCssBlocks(source) {
  const blocks = [];
  let cursor = 0;
  while (cursor < source.length) {
    while (/\s/.test(source[cursor] ?? "")) cursor += 1;
    if (cursor >= source.length) break;
    if (source.startsWith("/*", cursor)) {
      const end = source.indexOf("*/", cursor + 2);
      if (end < 0) throw new Error("unclosed CSS comment");
      blocks.push({
        kind: "comment",
        key: "",
        raw: source.slice(cursor, end + 2),
        start: cursor,
        end: end + 2,
      });
      cursor = end + 2;
      continue;
    }
    const open = source.indexOf("{", cursor);
    const semicolon = source.indexOf(";", cursor);
    if (open < 0 || (semicolon >= 0 && semicolon < open)) {
      const end = semicolon >= 0 ? semicolon + 1 : source.length;
      const raw = source.slice(cursor, end);
      const key = raw.trim().replace(/;\s*$/, "");
      if (key !== "") {
        blocks.push({ kind: "statement", key, raw, start: cursor, end });
      }
      cursor = end;
      continue;
    }
    const close = findMatchingBrace(source, open);
    const raw = source.slice(cursor, close + 1);
    const prelude = source.slice(cursor, open).trim();
    blocks.push({ kind: "block", key: prelude, raw, start: cursor, end: close + 1 });
    cursor = close + 1;
  }
  return blocks;
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key?.startsWith("--") || value === undefined) {
      throw new Error("usage: css-rule-safe-merge.mjs --base base.css --incoming incoming.css --output output.css --take selector-list");
    }
    args[key.slice(2)] = value;
  }
  for (const required of ["base", "incoming", "output", "take"]) {
    if (!args[required]) throw new Error(`missing --${required}`);
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));
const take = new Set(
  args.take.split(",").map((selector) => selector.trim()).filter(Boolean),
);
const matchesTake = (key) =>
  key
    .split(",")
    .map((selector) => selector.trim())
    .some((selector) => [...take].some((prefix) => selector.startsWith(prefix)));
const baseSource = readFileSync(args.base, "utf8");
const base = parseCssBlocks(baseSource);
const incoming = parseCssBlocks(readFileSync(args.incoming, "utf8"));
const byKey = new Map(base.map((block, index) => [block.key, index]));
const replacements = [];
const appended = [];

for (const block of incoming) {
  if (block.kind !== "block" || !matchesTake(block.key)) continue;
  const existing = byKey.get(block.key);
  if (existing === undefined) {
    byKey.set(block.key, null);
    appended.push(block.raw);
  } else if (existing !== null) {
    const target = base[existing];
    replacements.push({ start: target.start, end: target.end, raw: block.raw });
  }
}

let output = baseSource;
for (const replacement of replacements.sort((a, b) => b.start - a.start)) {
  output =
    output.slice(0, replacement.start) +
    replacement.raw +
    output.slice(replacement.end);
}
if (appended.length > 0) {
  output = output.replace(/\s*$/, "\n\n") + appended.join("\n\n") + "\n";
}
writeFileSync(args.output, output, "utf8");
console.log(
  `rule-safe CSS merge: ${base.length} base blocks, ${replacements.length} replaced, ${appended.length} appended`,
);
