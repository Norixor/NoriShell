import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import type { PluginUiContribution } from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import NvxPluginUiDocument from "./NvxPluginUiDocument.vue";

type ActionReply = { contribution: PluginUiContribution; closeDialogOnSuccess?: boolean } | null;

function contribution(): PluginUiContribution {
  return {
    pluginId: "com.norishell.fixture",
    pluginName: "Fixture",
    artifactFingerprintSha256: "1".repeat(64),
    packageSha256: "a".repeat(64),
    instanceGeneration: "1",
    stateVersion: "1",
    contributionRevision: "1",
    target: {
      targetId: "plugins.page",
      surfaceKind: "inline",
      contextHandle: "019d0000-0000-4000-8000-000000000001",
      targetRevision: "1",
      displayLabel: null,
    },
    document: {
      schemaVersion: 1,
      rootNodeId: "root",
      nodes: [
        {
          kind: "stack",
          nodeId: "root",
          direction: "vertical",
          align: "stretch",
          gap: 8,
          children: ["name", "run"],
        },
        {
          kind: "textField",
          nodeId: "name",
          fieldId: "name",
          label: "Name",
          value: "NoriShell",
          placeholder: null,
          fieldKind: "text",
          required: true,
          disabled: false,
        },
        {
          kind: "button",
          nodeId: "run",
          actionId: "run",
          label: "Run",
          icon: "play",
          variant: "primary",
          disabled: false,
        },
      ],
    },
  };
}

describe("NvxPluginUiDocument", () => {
  it("renders bounded shared tabs, tree, chart and editor nodes through the host action boundary", async () => {
    const page = contribution();
    page.document = {
      schemaVersion: 1,
      rootNodeId: "root",
      nodes: [
        {
          kind: "stack", nodeId: "root", direction: "vertical", align: "stretch", gap: 8,
          children: ["tabs", "tree", "chart", "editor"],
        },
        {
          kind: "tabs", nodeId: "tabs", label: "Views", tabs: [
            { id: "active", label: "Active", children: ["activeText"] },
            { id: "history", label: "History", children: ["historyText"] },
          ],
        },
        { kind: "text", nodeId: "activeText", text: "Active service", style: "body", tone: "neutral" },
        { kind: "text", nodeId: "historyText", text: "Service history", style: "body", tone: "neutral" },
        {
          kind: "tree", nodeId: "tree", label: "Services", items: [{
            id: "system", label: "System", children: [{
              id: "nginx", label: "nginx.service", actionId: "inspect",
            }],
          }],
        },
        {
          kind: "chart", nodeId: "chart", label: "CPU", chartKind: "line", labels: ["Now", "Later"],
          series: [{ label: "Load", values: [10, 30] }],
        },
        {
          kind: "editor", nodeId: "editor", fieldId: "filter", label: "Filter", value: "service=nginx",
          language: "ini", readOnly: false,
        },
      ],
    };
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    expect(wrapper.get(".plugin-ui-tabs").attributes("aria-label")).toBe("Views");
    expect(wrapper.text()).toContain("Active service");
    await wrapper.findAll('[role="tab"]')[1]?.trigger("click");
    expect(wrapper.text()).toContain("Service history");
    expect(wrapper.get('[role="img"]')).toBeTruthy();

    await wrapper.get(".plugin-ui-tree__toggle").trigger("click");
    await wrapper.findAll(".plugin-ui-tree__item")[1]?.trigger("click");
    expect(wrapper.emitted("action")).toEqual([[
      "inspect",
      [{ fieldId: "filter", value: "service=nginx" }],
    ]]);
    wrapper.unmount();
  });

  it("keeps wide table data readable and keyboard reachable without truncating paths", async () => {
    const page = contribution();
    const path = `/var/log/${"long-directory/".repeat(20)}service.log`;
    page.document = {
      schemaVersion: 1,
      rootNodeId: "table",
      nodes: [{
        kind: "table", nodeId: "table", label: "Log files",
        columns: [
          { columnId: "path", label: "Path", width: 480 },
          { columnId: "state", label: "State", width: null },
          { columnId: "size", label: "Size", width: 1 },
        ],
        rows: [{ rowId: "file", cells: [path, "Ready\nLast checked just now", "12 KiB"], actionId: "inspect" }],
        emptyText: null,
      }],
    };
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    const region = wrapper.get('[role="region"]');
    expect(region.attributes("aria-label")).toBe("Log files");
    expect(region.attributes("tabindex")).toBe("0");
    expect(wrapper.get("table").attributes("style")).toContain("min-width: 672px");
    expect(wrapper.findAll("col")[0]?.attributes("style")).toContain("width: 480px");
    expect(wrapper.findAll("col")[2]?.attributes("style")).toContain("width: 48px");
    expect(wrapper.findAll("td")[0]?.element.textContent).toBe(path);
    expect(wrapper.findAll("td")[1]?.element.textContent).toBe("Ready\nLast checked just now");
    await wrapper.get("tbody tr").trigger("keydown", { key: "Enter" });
    expect(wrapper.emitted("action")).toEqual([["inspect", []]]);
    wrapper.unmount();
  });

  it("preserves complete log text as plain text in the keyboard-scrollable code surface", () => {
    const page = contribution();
    const log = `${"2026-09-08 12:00:00 INFO service running\n".repeat(120)}<script>not executable</script>\n${"x".repeat(2000)}`;
    page.document = {
      schemaVersion: 1, rootNodeId: "log",
      nodes: [{ kind: "code", nodeId: "log", text: log, language: null, wrap: false }],
    };
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false }, global: { plugins: [i18n] },
    });
    expect(wrapper.get("pre").attributes("tabindex")).toBe("0");
    expect(wrapper.get("code").element.textContent).toBe(log);
    expect(wrapper.find("script").exists()).toBe(false);
    wrapper.unmount();
  });

  it("retains independent values in a multi-field horizontal toolbar across revisions", async () => {
    const page = contribution();
    const fields = Array.from({ length: 6 }, (_, index) => `option${index}`);
    page.document = {
      schemaVersion: 1, rootNodeId: "toolbar",
      nodes: [
        { kind: "stack", nodeId: "toolbar", direction: "horizontal", align: "end", gap: 12, children: [...fields, "run"] },
        ...fields.map((fieldId) => ({
          kind: "textField" as const, nodeId: fieldId, fieldId, label: fieldId,
          value: "", placeholder: null, fieldKind: "text" as const, required: false, disabled: false,
        })),
        { kind: "button", nodeId: "run", actionId: "run", label: "Inspect", icon: null, variant: "primary", disabled: false },
      ],
    };
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false }, global: { plugins: [i18n] },
    });
    for (const [index, input] of wrapper.findAll("input").entries()) {
      await input.setValue(`/srv/${index}/${"long-path/".repeat(12)}`);
    }
    await wrapper.setProps({ contribution: { ...page, contributionRevision: "2" } });
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("action")).toEqual([["run", fields.map((fieldId, index) => ({
      fieldId, value: `/srv/${index}/${"long-path/".repeat(12)}`,
    }))]]);
    wrapper.unmount();
  });

  it("renders host-owned fields and emits a complete form snapshot", async () => {
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: contribution(), busy: false },
      global: { plugins: [i18n] },
    });
    await wrapper.get("input").setValue("Vincent");
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("action")).toEqual([["run", [{ fieldId: "name", value: "Vincent" }]]]);
  });

  it("applies changed row-selection and editor defaults while retaining drafts on unchanged defaults", async () => {
    const page = contribution();
    const root = page.document.nodes[0];
    if (root?.kind !== "stack") throw new Error("fixture root");
    root.children = ["resources", "name", "editor", "run"];
    page.document.nodes.push(
      {
        kind: "table", nodeId: "resources", label: "Resources",
        columns: [{ columnId: "name", label: "Name", width: null }],
        rows: [{ rowId: "resource", cells: ["nginx.service"], actionId: "select-resource" }], emptyText: null,
      },
      { kind: "textField", nodeId: "editor", fieldId: "editor", label: "Cron", value: "", placeholder: null, fieldKind: "multiline", required: false, disabled: false },
    );
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false }, global: { plugins: [i18n] },
    });
    await wrapper.get("input").setValue("unsubmitted resource");
    await wrapper.get("textarea").setValue("unsubmitted cron");
    await wrapper.get("tbody tr").trigger("click");
    expect(wrapper.emitted("action")?.[0]?.[0]).toBe("select-resource");

    const selected = structuredClone(page);
    selected.contributionRevision = "2";
    for (const node of selected.document.nodes) {
      if (node.kind === "textField" && node.fieldId === "name") node.value = "nginx.service";
      if (node.kind === "textField" && node.fieldId === "editor") node.value = "0 2 * * * /usr/local/bin/backup";
    }
    await wrapper.setProps({ contribution: selected });
    expect(wrapper.get<HTMLInputElement>("input").element.value).toBe("nginx.service");
    expect(wrapper.get<HTMLTextAreaElement>("textarea").element.value).toBe("0 2 * * * /usr/local/bin/backup");

    await wrapper.get("textarea").setValue("0 3 * * * /usr/local/bin/backup");
    await wrapper.setProps({ contribution: { ...selected, contributionRevision: "3" } });
    expect(wrapper.get<HTMLTextAreaElement>("textarea").element.value).toBe("0 3 * * * /usr/local/bin/backup");
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("action")?.[1]).toEqual(["run", [
      { fieldId: "editor", value: "0 3 * * * /usr/local/bin/backup" },
      { fieldId: "name", value: "nginx.service" },
    ]]);
    wrapper.unmount();
  });

  it("drops revoked or disabled select options instead of submitting stale host handles", async () => {
    const page = contribution();
    const root = page.document.nodes[0];
    if (root?.kind !== "stack") throw new Error("fixture root");
    root.children = ["host", "run"];
    page.document.nodes = page.document.nodes.filter((node) => node.nodeId !== "name");
    page.document.nodes.push({
      kind: "select", nodeId: "host", fieldId: "host", label: "Host", value: "host-a", disabled: false,
      options: [{ value: "host-a", label: "A", disabled: false }, { value: "host-b", label: "B", disabled: false }],
    });
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false }, global: { plugins: [i18n] },
    });
    wrapper.findComponent({ name: "NvxSelect" }).vm.$emit("update:modelValue", "host-b");
    const revoked = structuredClone(page);
    revoked.contributionRevision = "2";
    const host = revoked.document.nodes.find((node) => node.kind === "select");
    if (host?.kind !== "select") throw new Error("fixture host");
    host.options = [{ value: "host-a", label: "A", disabled: false }];
    await wrapper.setProps({ contribution: revoked });
    await wrapper.get(".nvx-button").trigger("click");
    expect(wrapper.emitted("action")?.[0]).toEqual(["run", [{ fieldId: "host", value: "host-a" }]]);

    const unavailable = structuredClone(revoked);
    unavailable.contributionRevision = "3";
    const unavailableHost = unavailable.document.nodes.find((node) => node.kind === "select");
    if (unavailableHost?.kind !== "select") throw new Error("fixture host");
    unavailableHost.options[0]!.disabled = true;
    await wrapper.setProps({ contribution: unavailable });
    await wrapper.get(".nvx-button").trigger("click");
    expect(wrapper.emitted("action")?.[1]).toEqual(["run", [{ fieldId: "host", value: "" }]]);
    wrapper.unmount();
  });

  it("submits a page password once to Core and clears the renderer value immediately", async () => {
    const page = contribution();
    page.target.targetId = "app.page";
    page.target.surfaceKind = "page";
    const root = page.document.nodes[0];
    if (!root || root.kind !== "stack") throw new Error("fixture root");
    root.children = ["name", "password", "run"];
    page.document.nodes.splice(2, 0, {
      kind: "textField",
      nodeId: "password",
      fieldId: "password",
      label: "Password",
      value: "",
      placeholder: null,
      fieldKind: "password",
      required: true,
      disabled: false,
    });

    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    await wrapper.get("input[type='text']").setValue("Vincent");
    const password = wrapper.get<HTMLInputElement>("input[type='password']");
    await password.setValue("plugin-secret");
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("action")).toEqual([["run", [
      { fieldId: "name", value: "Vincent" },
      { fieldId: "password", value: "plugin-secret" },
    ]]]);
    expect(password.element.value).toBe("");

    await wrapper.setProps({
      contribution: { ...page, contributionRevision: "2" },
    });
    expect(wrapper.get<HTMLInputElement>("input[type='text']").element.value).toBe("Vincent");
    expect(wrapper.get<HTMLInputElement>("input[type='password']").element.value).toBe("");
    await wrapper.get("input[type='password']").setValue("unsubmitted secret");
    await wrapper.setProps({ contribution: { ...page, contributionRevision: "3" } });
    expect(wrapper.get<HTMLInputElement>("input[type='password']").element.value).toBe("");
  });

  it("submits only fields from the action's own dialog", async () => {
    const page = contribution();
    page.target.targetId = "app.page";
    page.target.surfaceKind = "page";
    page.document = {
      schemaVersion: 1,
      rootNodeId: "root",
      nodes: [
        {
          kind: "stack",
          nodeId: "root",
          direction: "horizontal",
          align: "center",
          gap: 8,
          children: ["loginDialog", "registerDialog"],
        },
        {
          kind: "dialog",
          nodeId: "loginDialog",
          title: "Sign in",
          description: null,
          triggerLabel: "Sign in",
          closeLabel: "Close sign in",
          children: ["loginName", "loginPassword", "loginAction"],
        },
        {
          kind: "textField",
          nodeId: "loginName",
          fieldId: "loginName",
          label: "Email",
          value: "",
          placeholder: null,
          fieldKind: "text",
          required: true,
          disabled: false,
        },
        {
          kind: "textField",
          nodeId: "loginPassword",
          fieldId: "loginPassword",
          label: "Password",
          value: "",
          placeholder: null,
          fieldKind: "password",
          required: true,
          disabled: false,
        },
        {
          kind: "button",
          nodeId: "loginAction",
          actionId: "login",
          label: "Submit login",
          icon: null,
          variant: "primary",
          disabled: false,
        },
        {
          kind: "dialog",
          nodeId: "registerDialog",
          title: "Register",
          description: null,
          triggerLabel: "Register",
          closeLabel: "Close registration",
          children: ["registerName", "registerPassword", "registerAction"],
        },
        {
          kind: "textField",
          nodeId: "registerName",
          fieldId: "registerName",
          label: "Email",
          value: "",
          placeholder: null,
          fieldKind: "text",
          required: true,
          disabled: false,
        },
        {
          kind: "textField",
          nodeId: "registerPassword",
          fieldId: "registerPassword",
          label: "Password",
          value: "",
          placeholder: null,
          fieldKind: "password",
          required: true,
          disabled: false,
        },
        {
          kind: "button",
          nodeId: "registerAction",
          actionId: "register",
          label: "Submit registration",
          icon: null,
          variant: "primary",
          disabled: false,
        },
      ],
    };

    const wrapper = mount(NvxPluginUiDocument, {
      attachTo: document.body,
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    await wrapper.findAll("button").find((button) => button.text() === "Sign in")?.trigger("click");
    const dialog = document.querySelector<HTMLElement>("[role='dialog']");
    const inputs = dialog?.querySelectorAll<HTMLInputElement>("input");
    expect(inputs).toHaveLength(2);
    if (!inputs || !dialog) throw new Error("login dialog");
    inputs[0]!.value = "user@example.test";
    inputs[0]!.dispatchEvent(new Event("input", { bubbles: true }));
    inputs[1]!.value = "secret";
    inputs[1]!.dispatchEvent(new Event("input", { bubbles: true }));
    const submit = [...dialog.querySelectorAll<HTMLButtonElement>("button")]
      .find((button) => button.textContent?.includes("Submit login"));
    submit?.click();
    await wrapper.vm.$nextTick();

    expect(wrapper.emitted("action")).toEqual([["login", [
      { fieldId: "loginName", value: "user@example.test" },
      { fieldId: "loginPassword", value: "secret" },
    ]]]);
    wrapper.unmount();
  });

  it("opens a page dialog only from its host-rendered trigger and closes with Escape", async () => {
    const page = contribution();
    page.target.targetId = "app.page";
    page.target.surfaceKind = "page";
    page.document = {
      schemaVersion: 1,
      rootNodeId: "dialog",
      nodes: [
        {
          kind: "dialog",
          nodeId: "dialog",
          title: "Register",
          description: "Create an account",
          triggerLabel: "Open registration",
          closeLabel: "Close registration",
          children: ["message"],
        },
        {
          kind: "text",
          nodeId: "message",
          text: "Registration form",
          style: "body",
          tone: "neutral",
        },
      ],
    };
    const wrapper = mount(NvxPluginUiDocument, {
      attachTo: document.body,
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    expect(document.querySelector("[role='dialog']")).toBeNull();
    await wrapper.get("button").trigger("click");
    const dialog = document.querySelector<HTMLElement>("[role='dialog']");
    expect(dialog?.textContent).toContain("Registration form");
    dialog?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await wrapper.vm.$nextTick();
    expect(document.querySelector("[role='dialog']")).toBeNull();
    wrapper.unmount();
  });

  it("uses the leading page heading without promoting dialog or later content headings", async () => {
    const page = contribution();
    page.document = {
      schemaVersion: 1,
      rootNodeId: "root",
      nodes: [
        { kind: "stack", nodeId: "root", direction: "vertical", align: "stretch", gap: 24, children: ["account", "header", "section"] },
        { kind: "text", nodeId: "count", text: "42", style: "heading", tone: "neutral" },
        { kind: "dialog", nodeId: "account", title: "Account", description: null, triggerLabel: "Account", closeLabel: "Close", children: ["dialogHeading"] },
        { kind: "text", nodeId: "dialogHeading", text: "Account settings", style: "heading", tone: "neutral" },
        { kind: "grid", nodeId: "header", columns: 2, columnWeights: null, gap: 24, children: ["identity", "actions"] },
        { kind: "stack", nodeId: "identity", direction: "vertical", align: "start", gap: 8, children: ["breadcrumb", "title", "description"] },
        { kind: "text", nodeId: "breadcrumb", text: "Workspace", style: "caption", tone: "neutral" },
        { kind: "text", nodeId: "title", text: "Sync", style: "heading", tone: "neutral" },
        { kind: "text", nodeId: "description", text: "Synchronize saved connections.", style: "secondary", tone: "neutral" },
        { kind: "button", nodeId: "actions", actionId: "refresh", label: "Refresh", icon: "refresh", variant: "ghost", disabled: false },
        { kind: "section", nodeId: "section", title: "Saved connections", children: ["count"] },
      ],
    };
    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    expect(wrapper.find("h1").exists()).toBe(false);
    await wrapper.setProps({ contribution: { ...page, target: { ...page.target, surfaceKind: "page" } } });
    expect(wrapper.findAll("h1")).toHaveLength(1);
    expect(wrapper.get("h1").text()).toBe("Sync");
    expect(wrapper.get(".plugin-ui-node--section .plugin-ui-node__text--heading").element.tagName).toBe("P");
    await wrapper.findAll("button")[0]?.trigger("click");
    await flushPromises();
    expect(document.querySelector("[role='dialog'] h1")).toBeNull();
    wrapper.unmount();
  });

  it("does not infer a page title from headings after a table or a content section", () => {
    for (const firstContent of ["table", "section"] as const) {
      const page = contribution();
      page.target.surfaceKind = "page";
      page.document = {
        schemaVersion: 1,
        rootNodeId: "root",
        nodes: [
          { kind: "stack", nodeId: "root", direction: "vertical", align: "stretch", gap: 16, children: [firstContent, "count"] },
          ...(firstContent === "table" ? [{
            kind: "table" as const, nodeId: "table", label: "Saved data", columns: [{ columnId: "count", label: "Count", width: null }],
            rows: [{ rowId: "hosts", cells: ["42"], actionId: null }], emptyText: null,
          }] : [{ kind: "section" as const, nodeId: "section", title: "Saved data", children: [] }]),
          { kind: "text", nodeId: "count", text: "42", style: "heading", tone: "neutral" },
        ],
      };
      const wrapper = mount(NvxPluginUiDocument, {
        props: { contribution: page, busy: false },
        global: { plugins: [i18n] },
      });
      expect(wrapper.find("h1").exists()).toBe(false);
      wrapper.unmount();
    }
  });

  it("renders plugin-owned weighted page columns", () => {
    const page = contribution();
    page.target.targetId = "app.page";
    page.target.surfaceKind = "page";
    page.document = {
      schemaVersion: 1,
      rootNodeId: "root",
      nodes: [
        {
          kind: "grid",
          nodeId: "root",
          columns: 2,
          columnWeights: [3, 8],
          gap: 16,
          children: ["left", "right"],
        },
        { kind: "text", nodeId: "left", text: "Status", style: "body", tone: "neutral" },
        { kind: "text", nodeId: "right", text: "Conflicts", style: "body", tone: "neutral" },
      ],
    };

    const wrapper = mount(NvxPluginUiDocument, {
      props: { contribution: page, busy: false },
      global: { plugins: [i18n] },
    });
    expect(wrapper.get(".plugin-ui-node--grid").attributes("style"))
      .toContain("minmax(0, 3fr) minmax(0, 8fr)");
  });

  function menuContribution(): PluginUiContribution {
    const page = contribution();
    page.document = {
      schemaVersion: 1,
      rootNodeId: "accountMenu",
      nodes: [
        { kind: "menu", nodeId: "accountMenu", label: "Account", children: ["settings", "disabled", "divider", "reset"] },
        { kind: "dialog", nodeId: "settings", title: "Settings", description: null, triggerLabel: "Edit settings", closeLabel: "Close settings", children: ["name", "save"] },
        { kind: "textField", nodeId: "name", fieldId: "name", label: "Name", value: "Saved name", placeholder: null, fieldKind: "text", required: true, disabled: false },
        { kind: "button", nodeId: "save", actionId: "save", label: "Save", icon: null, variant: "primary", disabled: false },
        { kind: "button", nodeId: "disabled", actionId: "disabled", label: "Unavailable", icon: null, variant: "secondary", disabled: true },
        { kind: "divider", nodeId: "divider" },
        { kind: "button", nodeId: "reset", actionId: "reset", label: "Reset", icon: "warning", variant: "danger", disabled: false },
      ],
    };
    return page;
  }

  it("renders a floating account menu with keyboard navigation, Escape and outside dismissal", async () => {
    const wrapper = mount(NvxPluginUiDocument, {
      attachTo: document.body,
      props: { contribution: menuContribution(), busy: false },
      global: { plugins: [i18n] },
    });
    const trigger = wrapper.get<HTMLButtonElement>('[aria-haspopup="menu"]');
    expect(wrapper.find("details").exists()).toBe(false);
    expect(wrapper.get('[role="menu"]').isVisible()).toBe(false);
    trigger.element.focus();
    await trigger.trigger("keydown", { key: "ArrowDown" });
    await flushPromises();
    const items = wrapper.findAll<HTMLButtonElement>('[role="menuitem"]');
    expect(document.activeElement).toBe(items[0]?.element);
    expect(trigger.attributes("aria-expanded")).toBe("true");
    await items[0]?.trigger("keydown", { key: "ArrowDown" });
    expect(document.activeElement).toBe(items[2]?.element);
    await items[2]?.trigger("keydown", { key: "Home" });
    expect(document.activeElement).toBe(items[0]?.element);
    await items[0]?.trigger("keydown", { key: "Escape" });
    await flushPromises();
    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(trigger.element);
    await trigger.trigger("click");
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    await flushPromises();
    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(wrapper.emitted("action")).toBeUndefined();
    wrapper.unmount();
  });

  it("keeps a menu-triggered dialog alive and restores focus to the account trigger", async () => {
    const wrapper = mount(NvxPluginUiDocument, {
      attachTo: document.body,
      props: { contribution: menuContribution(), busy: false },
      global: { plugins: [i18n] },
    });
    const trigger = wrapper.get<HTMLButtonElement>('[aria-haspopup="menu"]');
    await trigger.trigger("click");
    await flushPromises();
    await wrapper.get('[role="menuitem"]').trigger("click");
    await flushPromises();
    expect(trigger.attributes("aria-expanded")).toBe("false");
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.textContent).toContain("Settings");
    const input = dialog?.querySelector<HTMLInputElement>("input");
    if (!input || !dialog) throw new Error("missing menu dialog");
    input.value = "Edited name";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    [...dialog.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Save")?.click();
    await flushPromises();
    expect(wrapper.emitted("action")).toEqual([["save", [{ fieldId: "name", value: "Edited name" }]]]);
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flushPromises();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger.element);
    wrapper.unmount();
  });

  it("closes the menu after actions without submitting hidden dialog fields", async () => {
    const wrapper = mount(NvxPluginUiDocument, {
      attachTo: document.body,
      props: { contribution: menuContribution(), busy: false },
      global: { plugins: [i18n] },
    });
    const trigger = wrapper.get<HTMLButtonElement>('[aria-haspopup="menu"]');
    await trigger.trigger("click");
    await wrapper.findAll('[role="menuitem"]')[2]?.trigger("click");
    await flushPromises();
    expect(wrapper.emitted("action")).toEqual([["reset", []]]);
    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(trigger.element);
    await trigger.trigger("click");
    await wrapper.setProps({ busy: true });
    expect(trigger.attributes("disabled")).toBeDefined();
    expect(trigger.attributes("aria-expanded")).toBe("false");
    wrapper.unmount();
  });
  it.each([
    { plugin: "tunnels", fieldId: "listenPort", kind: "number", saved: "18444", draft: "18445" },
    { plugin: "tasks", fieldId: "command", kind: "multiline", saved: "echo saved", draft: "echo edited" },
  ] as const)("restores $plugin preset fields even when the authoritative value equals the old default", async ({ plugin, fieldId, kind, saved, draft }) => {
    const page = contribution();
    page.pluginId = `test.${plugin}`;
    page.document.nodes = [
      { kind: "stack", nodeId: "root", direction: "vertical", align: "stretch", gap: 8, children: ["presets", "field"] },
      { kind: "table", nodeId: "presets", label: "Presets", columns: [{ columnId: "name", label: "Name", width: null }],
        rows: [{ rowId: "qa", cells: ["Native QA"], actionId: `${plugin}:load:qa` }], emptyText: null },
      { kind: "textField", nodeId: "field", fieldId, fieldKind: kind, label: "Value", value: saved, placeholder: null, required: false, disabled: false },
    ];
    let finish!: (result: ActionReply) => void;
    const requestAction = vi.fn(() => new Promise<ActionReply>((resolve) => { finish = resolve; }));
    const wrapper = mount(NvxPluginUiDocument, { props: { contribution: page, busy: false, requestAction }, global: { plugins: [i18n] } });
    const field = wrapper.get("input,textarea");
    await field.setValue(draft);
    await wrapper.get("tbody tr").trigger("click");
    await flushPromises();
    expect(requestAction).toHaveBeenCalledWith(`${plugin}:load:qa`, [{ fieldId, value: draft }]);
    const restored = { ...page, contributionRevision: "2" };
    await wrapper.setProps({ contribution: restored });
    expect(field.element).toHaveProperty("value", draft);
    finish({ contribution: restored });
    await flushPromises();
    expect(field.element).toHaveProperty("value", saved);
    await field.setValue(draft);
    await wrapper.setProps({ contribution: { ...restored, contributionRevision: "3" } });
    expect(field.element).toHaveProperty("value", draft);
    wrapper.unmount();
  });

  it("keeps edits made after submission and ignores failed or expired action replies", async () => {
    const page = contribution();
    const pending: Array<(result: ActionReply) => void> = [];
    const requestAction = vi.fn(() => new Promise<ActionReply>((resolve) => pending.push(resolve)));
    const wrapper = mount(NvxPluginUiDocument, { props: { contribution: page, busy: false, requestAction }, global: { plugins: [i18n] } });
    const field = wrapper.get("input");
    await field.setValue("submitted");
    await wrapper.get("button").trigger("click");
    await flushPromises();
    await field.setValue("edited while awaiting response");
    const response = structuredClone(page);
    response.contributionRevision = "2";
    const node = response.document.nodes.find((item) => item.kind === "textField");
    if (node?.kind !== "textField") throw new Error("fixture field");
    node.value = "authoritative result";
    await wrapper.setProps({ contribution: response });
    expect(field.element).toHaveProperty("value", "edited while awaiting response");
    pending.shift()!({ contribution: response });
    await flushPromises();
    expect(field.element).toHaveProperty("value", "edited while awaiting response");

    await wrapper.get("button").trigger("click");
    await flushPromises();
    pending.shift()!(null);
    await flushPromises();
    expect(field.element).toHaveProperty("value", "edited while awaiting response");

    await wrapper.get("button").trigger("click");
    await flushPromises();
    const replacement = structuredClone(response);
    replacement.target.contextHandle = "new-context";
    const replacementField = replacement.document.nodes.find((item) => item.kind === "textField");
    if (replacementField?.kind !== "textField") throw new Error("fixture replacement field");
    replacementField.value = "new context value";
    await wrapper.setProps({ contribution: replacement });
    pending.shift()!({ contribution: { ...response, contributionRevision: "3" } });
    await flushPromises();
    expect(field.element).toHaveProperty("value", "new context value");
    wrapper.unmount();
  });

  it("applies an explicit dialog result only to its submitted field scope", async () => {
    const page = contribution();
    page.document.nodes = [
      { kind: "stack", nodeId: "root", direction: "vertical", align: "stretch", gap: 8, children: ["outside", "dialog"] },
      { kind: "textField", nodeId: "outside", fieldId: "outside", fieldKind: "text", label: "Outside", value: "outside default", placeholder: null, required: false, disabled: false },
      { kind: "dialog", nodeId: "dialog", title: "Editor", triggerLabel: "Edit", closeLabel: "Close", description: null, children: ["inside", "save"] },
      { kind: "textField", nodeId: "inside", fieldId: "inside", fieldKind: "text", label: "Inside", value: "inside default", placeholder: null, required: false, disabled: false },
      { kind: "button", nodeId: "save", actionId: "save-dialog", label: "Save", variant: "primary", icon: null, disabled: false },
    ];
    let finish!: (result: ActionReply) => void;
    const requestAction = vi.fn(() => new Promise<ActionReply>((resolve) => { finish = resolve; }));
    const wrapper = mount(NvxPluginUiDocument, { props: { contribution: page, busy: false, requestAction },
      global: { plugins: [i18n], stubs: { Teleport: true } } });
    await wrapper.get("input").setValue("outside draft");
    await wrapper.get("button").trigger("click");
    await wrapper.get('[role="dialog"] input').setValue("inside draft");
    await wrapper.findAll('[role="dialog"] button').find((button) => button.text() === "Save")!.trigger("click");
    await flushPromises();
    expect(requestAction).toHaveBeenCalledWith("save-dialog", [{ fieldId: "inside", value: "inside draft" }]);
    const result = structuredClone(page);
    result.contributionRevision = "2";
    const outside = result.document.nodes.find((item) => item.nodeId === "outside");
    if (outside?.kind !== "textField") throw new Error("fixture outside field");
    outside.value = "unrelated server default";
    await wrapper.setProps({ contribution: result });
    finish({ contribution: result });
    await flushPromises();
    expect(wrapper.get("input").element).toHaveProperty("value", "outside draft");
    expect(wrapper.get('[role="dialog"] input').element).toHaveProperty("value", "inside default");

    await wrapper.findAll('[role="dialog"] button').find((button) => button.text() === "Save")!.trigger("click");
    await flushPromises();
    finish(null);
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);

    await wrapper.findAll('[role="dialog"] button').find((button) => button.text() === "Save")!.trigger("click");
    await flushPromises();
    const success = { ...result, contributionRevision: "3" };
    await wrapper.setProps({ contribution: success });
    finish({ contribution: success, closeDialogOnSuccess: true });
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(wrapper.get("input").element).toHaveProperty("value", "outside draft");
    wrapper.unmount();
  });

});
