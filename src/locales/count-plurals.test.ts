import { describe, expect, it } from "vitest";

import { i18n } from ".";

/** English count copy must pluralize; Chinese keeps one form and the plural argument must stay harmless. */
describe("count copy plural forms", () => {
  const cases: [key: string, singular: string, plural: string][] = [
    ["sshTerminal.paneCount", "1 Pane", "2 Panes"],
    ["sftp.bytes", "1 byte", "0 bytes"],
    ["sshSession.heartbeat.transportFailures", "SSH keepalive failed 1 time in a row", "SSH keepalive failed 3 times in a row"],
    ["desktopPreferences.recentCount", "1 item", "5 items"],
    ["terminalEnhancements.paste.lines", "1 line", "12 lines"],
    ["mobile.tabsCount", "1 tab", "4 tabs"],
    ["sshHosts.opensshImport.imported", "Atomically imported 1 host", "Atomically imported 2 hosts"],
  ];

  it("selects singular and plural English forms from the count", () => {
    i18n.global.locale.value = "en";
    for (const [key, singular, plural] of cases) {
      const count = Number(plural.match(/\d+/)![0]);
      expect(i18n.global.t(key, 1)).toBe(singular);
      expect(i18n.global.t(key, count)).toBe(plural);
    }
    expect(i18n.global.t("sshHosts.groupSection", { label: "Prod", count: 1 }, 1)).toBe("Prod · 1 host");
    expect(i18n.global.t("sshHosts.groupSection", { label: "Prod", count: 3 }, 3)).toBe("Prod · 3 hosts");
  });

  it("keeps the single Chinese form for every count", () => {
    i18n.global.locale.value = "zh-CN";
    expect(i18n.global.t("sshTerminal.paneCount", 1)).toBe("1 个 Pane");
    expect(i18n.global.t("sshTerminal.paneCount", 2)).toBe("2 个 Pane");
    expect(i18n.global.t("mobile.tabsCount", 3)).toBe("3 个标签页");
    i18n.global.locale.value = "en";
  });
});
