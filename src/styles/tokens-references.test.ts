import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const root = process.cwd();
const sourceExtensions = /\.(vue|css|ts)$/;

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return sourceExtensions.test(entry.name) && !entry.name.endsWith(".test.ts") ? [path] : [];
  });
}

function captures(source: string, pattern: RegExp) {
  return [...source.matchAll(pattern)].flatMap((match) => (match[1] ? [match[1]] : []));
}

describe("design token references", () => {
  // An undefined custom property makes the whole declaration invalid, so a
  // misspelled token silently drops borders, colors or radii instead of failing.
  it("only references --nvx-* variables that are defined somewhere", () => {
    const html = readdirSync(root).filter((name) => name.endsWith(".html")).map((name) => join(root, name));
    const sources = [...sourceFiles(join(root, "src")), ...html].map((path) => readFileSync(path, "utf8"));
    const defined = new Set<string>();
    const referenced = new Set<string>();
    for (const source of sources) {
      // CSS declarations plus string literals written at runtime (app theme, setProperty).
      captures(source, /(--nvx-[a-z0-9-]+)\s*:/g).forEach((name) => defined.add(name));
      captures(source, /["'`](--nvx-[a-z0-9-]+)/g).forEach((name) => defined.add(name));
      captures(source, /var\(\s*(--nvx-[a-z0-9-]+)/g).forEach((name) => referenced.add(name));
    }
    expect([...referenced].filter((name) => !defined.has(name)).sort()).toEqual([]);
  });
});
