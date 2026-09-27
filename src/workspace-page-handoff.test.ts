import { createPinia } from "pinia";
import { describe, expect, it } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";

import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { createPageHandoff } from "./workspace-page-handoff";

describe("Page Tab handoff", () => {
  it("imports the built-in New Page Tab into another window", async () => {
    const store = useWorkspaceTabsStore(createPinia());
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: "/new", component: { template: "<div />" } }],
    });
    const handoff = createPageHandoff(store, router);
    const tab = {
      groupId: "page:newPage",
      pageType: "newPage" as const,
      route: "/new",
      labelKey: "newWorkspace.title",
      label: "",
      iconName: null,
    };

    await handoff.import(tab.groupId, tab);

    expect(store.pageTabs).toEqual([tab]);
  });
});
