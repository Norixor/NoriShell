import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";

const client = vi.hoisted(() => ({
  createVault: vi.fn(),
  unlockVault: vi.fn(),
  disableVaultAutoUnlock: vi.fn(),
  enableVaultAutoUnlock: vi.fn(),
  fetchVaultStatus: vi.fn(),
  lockVault: vi.fn(),
}));

vi.mock("../core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("../core-api/client")>(),
  createVault: client.createVault,
  unlockVault: client.unlockVault,
  disableVaultAutoUnlock: client.disableVaultAutoUnlock,
  enableVaultAutoUnlock: client.enableVaultAutoUnlock,
  fetchVaultStatus: client.fetchVaultStatus,
  lockVault: client.lockVault,
}));
vi.mock("../terminal-fonts", () => ({ detectInstalledTerminalFonts: vi.fn(async () => []) }));

import SettingsView from "./SettingsView.vue";

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

  it("creates a missing Vault explicitly with matching confirmation", async () => {
    const missing = { state: "missing", vaultId: null, revision: null, entryCount: null, unlockPolicy: "currentSession", autoUnlockFailure: null };
    client.fetchVaultStatus.mockResolvedValue(missing);
    client.createVault.mockResolvedValue({ ...missing, state: "unlocked" });
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();
    const open = wrapper.findAll("button").find((button) => button.text() === "Create Vault")!;
    await open.trigger("click");
    await flushPromises();
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!;
    expect(dialog.textContent).toContain("Create encrypted Vault");
    expect(dialog.textContent).toContain("same Vault password");
    const password = dialog.querySelector<HTMLInputElement>("#settings-vault-password")!;
    const confirmation = dialog.querySelector<HTMLInputElement>("#settings-vault-confirmation")!;
    password.value = "new-vault-password";
    password.dispatchEvent(new Event("input", { bubbles: true }));
    confirmation.value = "wrong-password";
    confirmation.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPromises();
    const submit = Array.from(dialog.querySelectorAll<HTMLButtonElement>("button")).find((button) => button.textContent?.trim() === "Create Vault")!;
    expect(submit.disabled).toBe(true);
    confirmation.value = "new-vault-password";
    confirmation.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPromises();
    submit.click();
    await flushPromises();
    expect(client.createVault).toHaveBeenCalledWith("new-vault-password", "new-vault-password");
    expect(client.unlockVault).not.toHaveBeenCalled();
    expect(document.querySelector("#settings-vault-password")).toBeNull();
    wrapper.unmount();
  });

  it("does not open an unlock prompt when fresh Vault status is unavailable", async () => {
    const wrapper = await mountSettings();
    await wrapper.vm.$router.push("/settings?section=vault");
    await flushPromises();
    client.fetchVaultStatus.mockRejectedValue(new Error("unavailable"));
    // A locked projection may become unavailable before an explicit action.
    await wrapper.vm.$router.push("/settings?section=application");
    wrapper.unmount();
    client.fetchVaultStatus.mockResolvedValueOnce({ state: "locked", unlockPolicy: "currentSession" });
    const second = await mountSettings();
    await second.vm.$router.push("/settings?section=vault");
    await flushPromises();
    await second.findAll("button").find((button) => button.text() === "Unlock Vault")!.trigger("click");
    await flushPromises();
    expect(document.querySelector("#settings-vault-password")).toBeNull();
    expect(second.text()).toContain("temporarily unavailable");
    expect(client.unlockVault).not.toHaveBeenCalled();
    second.unmount();
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
    expect(wrapper.vm.$router.currentRoute.value.path).toBe("/settings/identities");
    wrapper.unmount();
  });

  it("shows the explicit Vault unlock policy instead of timed lock choices", async () => {
    const wrapper = await mountSettings();

    const vault = wrapper.findAll("button")
      .find((button) => button.text().includes("Encrypted Vault"));
    expect(vault).toBeDefined();
    await vault!.trigger("click");

    expect(wrapper.text()).toContain("Until this app exits");
    expect(wrapper.text()).toContain("Lock now");
    expect(wrapper.text()).not.toContain("30 minutes");
    expect(wrapper.text()).not.toContain("30 days");
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

  it("persists startup, new-terminal, and single-Pane tab-close behavior choices", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({ locale: "en" }));
    const wrapper = await mountSettings();

    const startup = wrapper.get('[aria-label="After startup"]');
    expect(startup.text()).toContain("Load terminal history");
    await startup.trigger("click");
    const welcomeStartup = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((option) => option.textContent?.includes("Show welcome page"));
    welcomeStartup?.click();
    await flushPromises();

    const newTerminal = wrapper.get('[aria-label="When creating a terminal"]');
    expect(newTerminal.text()).toContain("Show welcome page");
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
});
