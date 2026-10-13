import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";
import { handleMobileBack, mobileBackStackDepth, resetMobileOverlaysForTests } from "../mobile-overlays";

const client = vi.hoisted(() => ({
  secureVault: vi.fn(),
  createVault: vi.fn(),
  unlockVault: vi.fn(),
  disableVaultAutoUnlock: vi.fn(),
  enableVaultAutoUnlock: vi.fn(),
  fetchVaultStatus: vi.fn(),
  lockVault: vi.fn(),
  listCredentialRefs: vi.fn(),
  listIdentities: vi.fn(),
  listKnownHosts: vi.fn(),
}));

vi.mock("../core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("../core-api/client")>(),
  createVault: client.createVault,
  unlockVault: client.unlockVault,
  disableVaultAutoUnlock: client.disableVaultAutoUnlock,
  enableVaultAutoUnlock: client.enableVaultAutoUnlock,
  fetchVaultStatus: client.fetchVaultStatus,
  lockVault: client.lockVault,
  listCredentialRefs: client.listCredentialRefs,
  listIdentities: client.listIdentities,
  listKnownHosts: client.listKnownHosts,
}));
vi.mock("../core-api/secure-vault-client", () => ({ requestSecureVault: client.secureVault }));
const shells = vi.hoisted(() => ({ catalog: vi.fn() }));
vi.mock("../core-api/local-terminal-shells", () => ({ getLocalTerminalShellCatalog: shells.catalog }));
vi.mock("../terminal-fonts", () => ({ detectInstalledTerminalFonts: vi.fn(async () => []) }));
const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

import SettingsView from "./SettingsView.vue";
import { useUiStore } from "../stores/ui";

async function mountSettings() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/settings", component: SettingsView },
      { path: "/plugins", component: { template: "<div>Plugins</div>" } },
      { path: "/settings/identities", component: { template: "<div>Identities</div>" } },
      { path: "/known-hosts", component: { template: "<div>Known hosts</div>" } },
    ],
  });
  await router.push("/settings");
  await router.isReady();
  const wrapper = mount(SettingsView, {
    global: { plugins: [createPinia(), router, i18n] },
  });
  await flushPromises();
  return wrapper;
}

describe("Settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetMobileOverlaysForTests();
    client.secureVault.mockReset().mockResolvedValue(true);
    localStorage.clear();
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({ locale: "en" }));
    i18n.global.locale.value = "en";
    client.fetchVaultStatus.mockResolvedValue({
      state: "unlocked",
      vaultId: "01991c54-9122-7000-8000-000000000001",
      revision: 1,
      entryCount: 0,
      unlockPolicy: "currentSession",
      autoUnlockFailure: null,
    });
    client.listIdentities.mockResolvedValue([]);
    client.listCredentialRefs.mockResolvedValue([]);
    client.listKnownHosts.mockResolvedValue([]);
    shells.catalog.mockReset().mockResolvedValue({
      defaultShellName: "zsh",
      options: [
        { id: "path:/bin/bash", name: "bash", path: "/bin/bash" },
        { id: "path:/bin/zsh", name: "zsh", path: "/bin/zsh" },
      ],
    });
  });

  it("hides desktop-only controls on the phone shell and names Android Keystore for auto-unlock", async () => {
    host.mobile = true;
    try {
      const wrapper = await mountSettings();
      const navLabels = wrapper.findAll("nav button .settings-mobile-row__label").map((label) => label.text());
      expect(navLabels).not.toContain("Window and tray");
      expect(navLabels).toContain("Notifications");
      expect(navLabels).toContain("Keyboard shortcuts");
      await wrapper.get('nav button[data-section="application"]').trigger("click");
      await flushPromises();
      expect(wrapper.find('[aria-label="Interface zoom"]').exists()).toBe(false);
      expect(wrapper.find('[aria-label="Interface language"]').exists()).toBe(true);
      // The desktop deep link opens the notification subset: no window-close or tray fields.
      await wrapper.vm.$router.push("/settings?section=desktop");
      await flushPromises();
      expect(wrapper.find("#desktop-preferences-title").text()).toBe("Notifications");
      const labels = wrapper.findAll("label").map((label) => label.text());
      expect(labels).not.toContain("When closing the main window");
      expect(labels.some((label) => label.includes("Recent"))).toBe(false);
      expect(wrapper.text()).toContain("System notification permission");
      expect(wrapper.text()).not.toContain("Show resource status and issue indicators");
      expect(wrapper.text()).not.toContain("Recent connections in tray");
      // Application wording drops zoom and the mouse "click"; the transfer group names only notification rules.
      await wrapper.vm.$router.push("/settings?section=application");
      await flushPromises();
      expect(wrapper.text()).toContain("plus the interface language");
      expect(wrapper.text()).not.toContain("language and zoom");
      expect(wrapper.text()).toContain("tap the plus button in the top bar");
      await wrapper.vm.$router.push("/settings?section=transfer");
      await flushPromises();
      expect(wrapper.text()).toContain("Notification rules");
      expect(wrapper.text()).not.toContain("Window, tray, and notification rules");
      await wrapper.vm.$router.push("/settings?section=vault");
      await flushPromises();
      expect(wrapper.text()).toContain("Android Keystore auto-unlock stores only a random device key");
      await wrapper.get('[role="combobox"]').trigger("click");
      await flushPromises();
      expect(document.body.textContent).toContain("Use Android Keystore to auto-unlock");
      expect(document.body.textContent).not.toContain("Use the system secure store to auto-unlock");
      wrapper.unmount();
    } finally {
      host.mobile = false;
      document.body.innerHTML = "";
    }
  });

  it("shows the Android default terminal size and follows a size changed outside the field", async () => {
    host.mobile = true;
    try {
      const wrapper = await mountSettings();
      await wrapper.vm.$router.push("/settings?section=terminal");
      await flushPromises();
      const field = wrapper.get<HTMLInputElement>("#terminal-font-size");
      expect(field.element.value).toBe("11");
      // A pinch on a terminal saves through the same preference while Settings stays mounted.
      await useUiStore().setTerminalFontSize(15);
      await flushPromises();
      expect(field.element.value).toBe("15");
      wrapper.unmount();
    } finally {
      host.mobile = false;
      document.body.innerHTML = "";
    }
  });

  it("renders only the appearance section when selected", async () => {
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=appearance");
    await flushPromises();
    expect(wrapper.find("#app-theme-title").exists()).toBe(true);
    expect(wrapper.findAll("h2").map((heading) => heading.text())).not.toContain("Encrypted Vault");
    expect(wrapper.find('[aria-label="Unlock policy"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("opens the isolated Vault flow without rendering or submitting secrets in Settings", async () => {
    const missing = { state: "missing", vaultId: null, revision: null, entryCount: null, unlockPolicy: "currentSession", autoUnlockFailure: null };
    client.fetchVaultStatus.mockResolvedValue(missing);
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();
    client.secureVault.mockImplementation(async () => {
      client.fetchVaultStatus.mockResolvedValue({ ...missing, state: "unlocked" });
      return true;
    });
    await wrapper.findAll("button").find((button) => button.text() === "Create Vault")!.trigger("click");
    await flushPromises();
    expect(client.secureVault).toHaveBeenCalledWith("ensureUnlocked");
    expect(client.createVault).not.toHaveBeenCalled();
    expect(client.unlockVault).not.toHaveBeenCalled();
    expect(document.querySelector('input[type="password"]')).toBeNull();
    expect(wrapper.text()).toContain("Lock now");
    wrapper.unmount();
  });

  it("reports a rejected secure operation without falling back to a main-window password prompt", async () => {
    client.fetchVaultStatus.mockResolvedValue({ state: "locked", unlockPolicy: "currentSession" });
    client.secureVault.mockRejectedValue(new Error("secureVaultStateChanged"));
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();
    await wrapper.findAll("button").find((button) => button.text() === "Unlock Vault")!.trigger("click");
    await flushPromises();
    expect(client.secureVault).toHaveBeenCalledWith("ensureUnlocked");
    expect(document.querySelector('input[type="password"]')).toBeNull();
    expect(wrapper.text()).toContain(i18n.global.t("secureWindow.vault.failed"));
    expect(wrapper.text()).not.toContain("8 characters");
    expect(client.unlockVault).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("does not duplicate the first-level Plugins entry", async () => {
    const wrapper = await mountSettings();

    expect(wrapper.text()).not.toContain("Manage Plugins");
    wrapper.unmount();
  });

  it("exposes usable Identity and Known Hosts management entries", async () => {
    const wrapper = await mountSettings();

    const identities = wrapper.get('button[aria-label="Manage identities"]');
    const knownHosts = wrapper.get('button[aria-label="Manage known hosts"]');
    expect(identities.text()).toContain("SSH identities");
    expect(knownHosts.text()).toContain("Server host keys");

    for (const entry of wrapper.findAll(".settings-security-item")) {
      expect(entry.element.firstElementChild?.classList.contains("settings-security-item__icon"))
        .toBe(true);
      expect(entry.find(".settings-security-item__copy").exists()).toBe(true);
    }

    await identities.trigger("click");
    await flushPromises();
    expect(wrapper.vm.$router.currentRoute.value.path).toBe("/settings");
    expect(wrapper.text()).toContain("There are no SSH identities yet");
    expect(identities.attributes("aria-current")).toBe("page");

    await knownHosts.trigger("click");
    await flushPromises();
    expect(wrapper.vm.$router.currentRoute.value.path).toBe("/settings");
    expect(wrapper.text()).toContain("No server host keys have been trusted yet");
    wrapper.unmount();
  });

  it("shows the explicit Vault unlock policy instead of timed lock choices", async () => {
    const wrapper = await mountSettings();

    const vault = wrapper.findAll("button")
      .find((button) => button.text().includes("Encrypted Vault"));
    expect(vault).toBeDefined();
    await vault!.trigger("click");

    expect(wrapper.text()).toContain("Until this app exits");
    await wrapper.get('[role="combobox"]').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Save the Vault password locally and auto-unlock");
    expect(wrapper.text()).toContain("Lock now");
    expect(wrapper.text()).not.toContain("30 minutes");
    expect(wrapper.text()).not.toContain("30 days");
    wrapper.unmount();
  });

  it("opens the protected local-password flow when the local auto-unlock policy is selected", async () => {
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();

    const policy = wrapper.get('[role="combobox"]');
    await policy.trigger("click");
    const local = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Save the Vault password locally"));
    expect(local).toBeDefined();
    local!.click();
    await flushPromises();

    expect(client.secureVault).toHaveBeenCalledWith("enableLocalAutoUnlock");
    expect(client.enableVaultAutoUnlock).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reopens the failed local-password auto-unlock flow", async () => {
    const locked = {
      state: "locked",
      vaultId: "01991c54-9122-7000-8000-000000000001",
      revision: 1,
      entryCount: 0,
      unlockPolicy: "automaticLocal",
      autoUnlockFailure: "deviceKeyMissing",
    };
    client.fetchVaultStatus.mockResolvedValue(locked);
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();

    expect(wrapper.text()).toContain("private local file");
    await wrapper.findAll("button").find((button) => button.text() === "Enable again")!.trigger("click");
    await flushPromises();

    expect(client.secureVault.mock.calls.map(([mode]) => mode)).toEqual(["enableLocalAutoUnlock"]);
    wrapper.unmount();
  });

  it("enables the local-password mode with one protected password prompt while locked", async () => {
    client.fetchVaultStatus.mockResolvedValue({
      state: "locked",
      unlockPolicy: "currentSession",
      autoUnlockFailure: null,
    });
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();

    await wrapper.get('[role="combobox"]').trigger("click");
    const local = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Save the Vault password locally"));
    local!.click();
    await flushPromises();

    expect(client.secureVault.mock.calls.map(([mode]) => mode)).toEqual(["enableLocalAutoUnlock"]);
    expect(wrapper.get('[role="combobox"]').text()).toContain("Until this app exits");
    wrapper.unmount();
  });

  it("uses the saved local password for an explicit unlock click", async () => {
    client.fetchVaultStatus.mockResolvedValue({
      state: "locked",
      unlockPolicy: "automaticLocal",
      autoUnlockFailure: null,
    });
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();

    expect(wrapper.text()).toContain("Lock ends this session only");
    await wrapper.findAll("button").find((button) => button.text() === "Unlock Vault")!.trigger("click");
    await flushPromises();

    expect(client.secureVault.mock.calls.map(([mode]) => mode)).toEqual(["unlockSavedLocal"]);
    wrapper.unmount();
  });

  it("switches between application and terminal preferences without changing routes", async () => {
    const wrapper = await mountSettings();

    expect(wrapper.find('[aria-labelledby="application-preferences-title"]').exists()).toBe(true);
    const terminal = wrapper.findAll("button")
      .find((button) => button.text().includes("Terminal preferences"));
    expect(terminal).toBeDefined();

    await terminal!.trigger("click");

    expect(wrapper.find('[aria-labelledby="terminal-appearance-title"]').exists()).toBe(true);
    expect(wrapper.vm.$router.currentRoute.value.path).toBe("/settings");
    wrapper.unmount();
  });

  it("shows About inside Settings with the app identity and GitHub address", async () => {
    const wrapper = await mountSettings();
    const about = wrapper.findAll("button")
      .find((button) => button.text() === "About");
    expect(about).toBeDefined();

    await about!.trigger("click");
    await flushPromises();

    expect(wrapper.get('[aria-labelledby="about-settings-title"]').text()).toContain("NoriShell");
    expect(wrapper.text()).toContain("github.com/Norixor/NoriShell");
    expect(wrapper.vm.$router.currentRoute.value.path).toBe("/settings");
    wrapper.unmount();
  });

  it("persists startup, header density, new-terminal, and single-Pane tab-close choices", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({ locale: "en" }));
    const wrapper = await mountSettings();

    const startup = wrapper.get('[aria-label="After startup"]');
    expect(startup.text()).toContain("Show overview page");
    await startup.trigger("click");
    expect(Array.from(document.querySelectorAll<HTMLElement>('[role="option"]')).map((option) => option.textContent?.trim()))
      .toEqual(["Show overview page", "Show welcome page", "Load terminal history"]);
    const welcomeStartup = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Show welcome page"));
    welcomeStartup?.click();
    await flushPromises();

    const newTerminal = wrapper.get('[aria-label="When creating a page"]');
    expect(newTerminal.text()).toContain("Show New Page");
    await newTerminal.trigger("click");
    const localTerminal = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Open local terminal"));
    localTerminal?.click();
    await flushPromises();

    const closeBehavior = wrapper.get('[aria-label="When disconnecting and closing a tab"]');
    expect(closeBehavior.text()).toContain("Always confirm");
    await closeBehavior.trigger("click");
    const closeDirectly = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Skip confirmation for one Pane"));
    closeDirectly?.click();
    await flushPromises();

    expect(JSON.parse(localStorage.getItem("norishell.ui.preferences.v1") ?? "{}")).toMatchObject({
      terminalStartupBehavior: "welcome",
      newTerminalBehavior: "localTerminal",
      singlePaneTabCloseBehavior: "closeDirectly",
    });
    wrapper.unmount();
  });

  describe("local terminal shell", () => {
    const shellSelect = (wrapper: Awaited<ReturnType<typeof mountSettings>>) => wrapper.get('[aria-label="Local terminal shell"]');
    const optionLabels = () => Array.from(document.querySelectorAll<HTMLElement>('[role="option"]')).map((option) => option.textContent?.trim());
    const choose = async (label: string) => {
      Array.from(document.querySelectorAll<HTMLElement>('[role="option"]')).find((option) => option.textContent?.includes(label))?.click();
      await flushPromises();
    };
    const storedShell = () => JSON.parse(localStorage.getItem("norishell.ui.preferences.v1") ?? "{}").localTerminalShell;

    it("lists the system default, detected shells and a custom path, and saves a catalog choice immediately", async () => {
      const wrapper = await mountSettings();
      const select = shellSelect(wrapper);
      expect(select.text()).toContain("System default (zsh)");
      await select.trigger("click");
      expect(optionLabels()).toEqual(["System default (zsh)", "bash", "zsh", "Custom path..."]);
      await choose("bash");
      expect(useUiStore().localTerminalShell).toBe("path:/bin/bash");
      expect(storedShell()).toBe("path:/bin/bash");
      wrapper.unmount();
    });

    it("saves a trimmed custom path on Enter and rejects empty or control-character input without saving", async () => {
      const wrapper = await mountSettings();
      await shellSelect(wrapper).trigger("click");
      await choose("Custom path...");
      expect(storedShell()).toBe("default");
      const input = wrapper.get('input[aria-label="Shell executable path"]');

      await input.trigger("keydown", { key: "Enter" });
      expect(wrapper.text()).toContain("Enter the path of a shell executable.");
      await input.setValue("/opt/a\u0007b");
      await input.trigger("keydown", { key: "Enter" });
      expect(wrapper.text()).toContain("cannot contain control characters");
      expect(storedShell()).toBe("default");

      await input.setValue("  /opt/fish/bin/fish  ");
      await input.trigger("keydown", { key: "Enter" });
      await flushPromises();
      expect(useUiStore().localTerminalShell).toBe("path:/opt/fish/bin/fish");
      expect(storedShell()).toBe("path:/opt/fish/bin/fish");
      wrapper.unmount();
    });

    it("shows a saved path outside the catalog as a custom path with its value prefilled", async () => {
      localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({ locale: "en", localTerminalShell: "path:/opt/custom/sh" }));
      const wrapper = await mountSettings();
      expect(shellSelect(wrapper).text()).toContain("Custom path...");
      expect((wrapper.get('input[aria-label="Shell executable path"]').element as HTMLInputElement).value).toBe("/opt/custom/sh");
      wrapper.unmount();
    });

    it("keeps a fixed id the machine does not offer selected and warns that it is unavailable", async () => {
      localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({ locale: "en", localTerminalShell: "cmd" }));
      const wrapper = await mountSettings();
      expect(shellSelect(wrapper).text()).toContain("cmd (unavailable on this device)");
      expect(wrapper.text()).toContain("Opening a local terminal will fail");
      wrapper.unmount();
    });

    it("falls back to an empty catalog when it cannot be read and hides the row on the phone shell", async () => {
      shells.catalog.mockResolvedValue({ defaultShellName: "", options: [] });
      const wrapper = await mountSettings();
      await shellSelect(wrapper).trigger("click");
      expect(optionLabels()).toEqual(["System default", "Custom path..."]);
      wrapper.unmount();

      host.mobile = true;
      try {
        const phone = await mountSettings();
        await phone.get('nav button[data-section="application"]').trigger("click");
        await flushPromises();
        expect(phone.find('[aria-label="Local terminal shell"]').exists()).toBe(false);
        phone.unmount();
      } finally { host.mobile = false; }
    });
  });

  it("offers follow-system as the third application theme preference", async () => {
    const wrapper = await mountSettings();

    await wrapper.findAll("button").find((button) => button.text() === "Appearance")!.trigger("click");
    await flushPromises();
    const theme = wrapper.get('[aria-label="Appearance mode"]');
    await theme.trigger("click");
    const systemTheme = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("System"));
    systemTheme?.click();
    await flushPromises();

    expect(JSON.parse(localStorage.getItem("norishell.ui.preferences.v1") ?? "{}")).toMatchObject({
      themePreference: "system",
    });
    wrapper.unmount();
  });

  it("keeps the active theme and reports a local preference save failure", async () => {
    const wrapper = await mountSettings();
    const setItem = vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });

    await wrapper.findAll("button").find((button) => button.text() === "Appearance")!.trigger("click");
    await flushPromises();
    const theme = wrapper.get('[aria-label="Appearance mode"]');
    await theme.trigger("click");
    const darkTheme = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Dark"));
    darkTheme?.click();
    await flushPromises();

    expect(theme.text()).toContain("Light");
    expect(wrapper.text()).toContain("Appearance could not be saved. Please retry.");
    setItem.mockRestore();
    wrapper.unmount();
  });
  describe("phone list and detail levels", () => {
    beforeEach(() => { host.mobile = true; });
    afterEach(() => {
      host.mobile = false;
      document.body.innerHTML = "";
    });

    it("lists the sections in groups and opens one section in place of the list", async () => {
      const wrapper = await mountSettings();
      expect(wrapper.find(".settings-sidebar").exists()).toBe(false);
      expect(wrapper.find("main.settings-detail").exists()).toBe(false);
      expect(wrapper.findAll(".settings-mobile-list__group h2").map((heading) => heading.text()))
        .toEqual(["General", "Security"]);
      const vaultRow = wrapper.get('button[data-section="vault"]');
      expect(vaultRow.text()).toContain("Encrypted Vault");
      expect(vaultRow.find(".settings-mobile-row__summary").text()).toBe("Unlocked");
      expect(wrapper.get('button[data-section="knownHosts"]').text()).toContain("Strict verification");
      expect(wrapper.get('button[data-section="application"]').find(".settings-mobile-row__summary").text())
        .toBe("Language, startup and new tabs");

      await wrapper.get('button[data-section="terminal"]').trigger("click");
      await flushPromises();
      expect(wrapper.find(".settings-mobile-list").exists()).toBe(false);
      expect(wrapper.find('[aria-labelledby="terminal-appearance-title"]').exists()).toBe(true);
      expect(wrapper.find('button[aria-label="Back to settings"]').exists()).toBe(true);
      expect(wrapper.vm.$router.currentRoute.value.fullPath).toBe("/settings?section=terminal");

      await wrapper.get('button[aria-label="Back to settings"]').trigger("click");
      await flushPromises();
      expect(wrapper.find(".settings-mobile-list").exists()).toBe(true);
      expect(wrapper.find("main.settings-detail").exists()).toBe(false);
      expect(wrapper.get('button[data-section="terminal"]').attributes("aria-current")).toBe("true");
      expect(wrapper.vm.$router.currentRoute.value.fullPath).toBe("/settings");
      expect(mobileBackStackDepth()).toBe(0);
      wrapper.unmount();
    });

    it("returns from the detail to the list on the Android back key, then leaves the key to history", async () => {
      const wrapper = await mountSettings();
      await wrapper.get('button[data-section="about"]').trigger("click");
      await flushPromises();
      expect(wrapper.find('[aria-labelledby="about-settings-title"]').exists()).toBe(true);

      expect(handleMobileBack()).toBe(true);
      await flushPromises();
      expect(wrapper.find(".settings-mobile-list").exists()).toBe(true);
      expect(wrapper.find('[aria-labelledby="about-settings-title"]').exists()).toBe(false);
      expect(wrapper.vm.$router.currentRoute.value.fullPath).toBe("/settings");
      expect(handleMobileBack()).toBe(false);
      expect(wrapper.find(".settings-mobile-list").exists()).toBe(true);
      wrapper.unmount();
    });

    it("opens a deep-linked section directly and drops the query when it closes", async () => {
      const wrapper = await mountSettings();
      await wrapper.vm.$router.push("/settings?section=vault");
      await flushPromises();
      expect(wrapper.find(".settings-mobile-list").exists()).toBe(false);
      expect(wrapper.find(".settings-vault").exists()).toBe(true);
      expect(mobileBackStackDepth()).toBe(1);

      await wrapper.get('button[aria-label="Back to settings"]').trigger("click");
      await flushPromises();
      expect(wrapper.vm.$router.currentRoute.value.fullPath).toBe("/settings");
      expect(wrapper.find(".settings-mobile-list").exists()).toBe(true);

      // The same deep link works again because the list level dropped the query.
      await wrapper.vm.$router.push("/settings?section=vault");
      await flushPromises();
      expect(wrapper.find(".settings-vault").exists()).toBe(true);
      wrapper.unmount();
      expect(mobileBackStackDepth()).toBe(0);
    });
  });

  it("keeps the desktop sidebar without phone list rows or a back button", async () => {
    const wrapper = await mountSettings();
    expect(wrapper.find("aside.settings-sidebar").exists()).toBe(true);
    expect(wrapper.find(".settings-mobile-list").exists()).toBe(false);
    expect(wrapper.find(".settings-mobile-row").exists()).toBe(false);
    expect(wrapper.find('button[aria-label="Back to settings"]').exists()).toBe(false);
    expect(wrapper.find("main.settings-detail").exists()).toBe(true);
    await wrapper.findAll("button").find((button) => button.text() === "About")!.trigger("click");
    await flushPromises();
    // Desktop selection does not mirror into the URL or the back stack.
    expect(wrapper.vm.$router.currentRoute.value.fullPath).toBe("/settings");
    expect(mobileBackStackDepth()).toBe(0);
    wrapper.unmount();
  });
});
