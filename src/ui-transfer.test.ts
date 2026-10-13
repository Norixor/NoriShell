import { describe, expect, it } from "vitest";

import {
  DEFAULT_APPLICATION_PREFERENCES,
  customLocalTerminalShell,
  isLocalTerminalShell,
  localTerminalShellPathIssue,
  normalizeApplicationPreferences,
  validateApplicationPreferences,
} from "./ui-transfer";

describe("local terminal shell syntax", () => {
  it("accepts the default, the fixed Windows ids and absolute paths", () => {
    for (const value of ["default", "cmd", "windowsPowerShell", "powershell7", "gitBash", "path:/bin/zsh", "path:C:\\Program Files\\Git\\bin\\bash.exe"]) {
      expect(isLocalTerminalShell(value)).toBe(true);
    }
  });

  it("rejects unknown ids, empty or padded paths, control characters and over-long paths", () => {
    for (const value of ["", "Default", "fish", "path:", "path:   ", "path: /bin/zsh", "path:/bin/zsh ", "path:/bin/a\nb", `path:/${"a".repeat(1024)}`, 3, null]) {
      expect(isLocalTerminalShell(value)).toBe(false);
    }
    expect(isLocalTerminalShell(`path:${"a".repeat(1024)}`)).toBe(true);
  });

  it("reports why an entered path is rejected and trims it when building the value", () => {
    expect(localTerminalShellPathIssue("   ")).toBe("empty");
    expect(localTerminalShellPathIssue("a".repeat(1025))).toBe("tooLong");
    expect(localTerminalShellPathIssue("/bin/\u0000sh")).toBe("controlCharacter");
    expect(localTerminalShellPathIssue("/bin/sh")).toBeNull();
    expect(customLocalTerminalShell("  /bin/sh ")).toBe("path:/bin/sh");
  });
});

describe("application preferences with the shell field", () => {
  it("keeps older files without the field valid and fills the default when normalizing", () => {
    const legacy: Record<string, unknown> = { ...DEFAULT_APPLICATION_PREFERENCES };
    delete legacy.localTerminalShell;
    expect(validateApplicationPreferences(legacy)).toBe(true);
    expect(normalizeApplicationPreferences(legacy).localTerminalShell).toBe("default");
  });

  it("validates the field when present", () => {
    expect(validateApplicationPreferences({ ...DEFAULT_APPLICATION_PREFERENCES, localTerminalShell: "path:/bin/bash" })).toBe(true);
    expect(validateApplicationPreferences({ ...DEFAULT_APPLICATION_PREFERENCES, localTerminalShell: "bash" })).toBe(false);
    expect(validateApplicationPreferences({ ...DEFAULT_APPLICATION_PREFERENCES, localTerminalShell: 1 })).toBe(false);
  });
});
