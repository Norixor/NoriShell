/// <reference types="node" />

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { parseThemeDefinition } from "../../src/app-theme";

const root = join(process.cwd(), "examples/theme-plugins");

describe("pure-data theme examples", () => {
  it.each(["clear", "midnight", "sand"])("uses the frontend engine's strict definition validator for %s", (slug) => {
    const definition: unknown = JSON.parse(readFileSync(join(root, slug, "assets/theme.json"), "utf8"));
    expect(parseThemeDefinition(definition)).not.toBeNull();
  });
});
