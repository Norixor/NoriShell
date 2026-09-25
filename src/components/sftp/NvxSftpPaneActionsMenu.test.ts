import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { describe, expect, it } from "vitest";

import { i18n } from "../../locales";
import NvxSftpPaneActionsMenu from "./NvxSftpPaneActionsMenu.vue";

describe("NvxSftpPaneActionsMenu", () => {
  it("keeps the menu outside a clipped pane and closes on Escape", async () => {
    const wrapper = mount(NvxSftpPaneActionsMenu, {
      attachTo: document.body,
      props: {
        paneId: "pane-1",
        kind: "local",
        ready: true,
        hasSelection: false,
        singleSelection: false,
        pending: false,
        showHidden: false,
        foldersFirst: true,
        canSplitHorizontal: true,
        canSplitVertical: true,
        canClose: false,
      },
      global: { plugins: [i18n, createPinia()], stubs: { NvxPluginContributionSlot: true } },
    });

    await wrapper.get('[aria-haspopup="menu"]').trigger("click");
    const menu = document.querySelector<HTMLElement>('.sftp-pane-actions-menu__popover');
    expect(menu?.parentElement).toBe(document.body);
    expect(menu?.querySelectorAll('[role="menuitem"]')).toHaveLength(5);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await wrapper.vm.$nextTick();
    expect(document.querySelector('.sftp-pane-actions-menu__popover')).toBeNull();
    wrapper.unmount();
  });
});
