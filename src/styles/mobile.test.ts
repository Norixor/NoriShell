import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * Layout contracts of the phone stylesheet that happy-dom cannot measure. The
 * route row must have no minimum height: with the soft keyboard open in landscape
 * the content area is shorter than the desktop floor, and any floor pushes the
 * terminal under the helper-key row (clipped by `.app-main`, cursor row hidden).
 */
describe("mobile.css layout contracts", () => {
  // vitest runs from the project root; `import.meta.url` is not a file URL under its module runner.
  const css = readFileSync("src/styles/mobile.css", "utf8");

  function rule(selector: string): string {
    const start = css.indexOf(`${selector} {`);
    expect(start, `rule for ${selector}`).toBeGreaterThanOrEqual(0);
    return css.slice(start, css.indexOf("}", start));
  }

  it("gives the phone content row no minimum height so the helper keys always keep their room", () => {
    const main = rule('html[data-nvx-host="mobile"] .app-main');
    expect(main).toMatch(/grid-template-rows:\s*auto minmax\(0, 1fr\) auto auto auto;/);
    expect(main).not.toContain("120px");
  });

  it("keeps the phone root free of the desktop minimum window size", () => {
    const root = rule('html[data-nvx-host="mobile"]:has(#app),\nhtml[data-nvx-host="mobile"]:has(#app) body,\nhtml[data-nvx-host="mobile"] #app');
    expect(root).toMatch(/min-height:\s*0;/);
  });
});
