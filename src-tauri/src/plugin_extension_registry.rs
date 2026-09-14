//! Canonical registry of host-rendered plugin extension targets.
//!
//! Security and credential surfaces are intentionally absent. Adding a target
//! is a reviewed host change; plugins cannot invent target identifiers.

use norishell_core_api::{
    PluginCapability, PluginExtensionSurfaceKind, PluginExtensionTargetDefinition,
    PluginExtensionTargetId,
};

const TARGETS: &[(
    &str,
    PluginExtensionSurfaceKind,
    PluginCapability,
    bool,
    bool,
)] = &[
    (
        "plugins.page",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        false,
        true,
    ),
    (
        "app.header.actions",
        PluginExtensionSurfaceKind::Toolbar,
        PluginCapability::UiPanel,
        false,
        false,
    ),
    // Ordinary route-scoped content only; the renderer owns placement and budgets.
    (
        "app.content.before",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "app.content.after",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "app.content.sidebar",
        PluginExtensionSurfaceKind::Sidebar,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "app.content.footer",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "app.content.floating",
        PluginExtensionSurfaceKind::Overlay,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.tools",
        PluginExtensionSurfaceKind::Sidebar,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.header",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.footer",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.floating",
        PluginExtensionSurfaceKind::Overlay,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.toolbar",
        PluginExtensionSurfaceKind::Toolbar,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.sidebar",
        PluginExtensionSurfaceKind::Sidebar,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "terminal.contextMenu",
        PluginExtensionSurfaceKind::Menu,
        PluginCapability::UiPanel,
        true,
        false,
    ),
    (
        "terminal.annotation",
        PluginExtensionSurfaceKind::Overlay,
        PluginCapability::TerminalAnnotation,
        true,
        false,
    ),
    (
        "sftp.toolbar",
        PluginExtensionSurfaceKind::Toolbar,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "sftp.contextMenu",
        PluginExtensionSurfaceKind::Menu,
        PluginCapability::UiPanel,
        true,
        false,
    ),
    (
        "sftp.transfer.actions",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        false,
    ),
    (
        "hosts.toolbar",
        PluginExtensionSurfaceKind::Toolbar,
        PluginCapability::UiPanel,
        false,
        true,
    ),
    (
        "host.detail.tools",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        true,
        true,
    ),
    (
        "overview.toolbar",
        PluginExtensionSurfaceKind::Toolbar,
        PluginCapability::UiPanel,
        false,
        true,
    ),
    (
        "overview.card.actions",
        PluginExtensionSurfaceKind::Card,
        PluginCapability::UiPanel,
        true,
        false,
    ),
    (
        "tunnels.toolbar",
        PluginExtensionSurfaceKind::Toolbar,
        PluginCapability::UiPanel,
        false,
        true,
    ),
    (
        "settings.tools",
        PluginExtensionSurfaceKind::Inline,
        PluginCapability::UiPanel,
        false,
        true,
    ),
    (
        "commandPalette",
        PluginExtensionSurfaceKind::Menu,
        PluginCapability::UiPanel,
        false,
        true,
    ),
    (
        "app.navigation",
        PluginExtensionSurfaceKind::Navigation,
        PluginCapability::UiNavigation,
        false,
        false,
    ),
    (
        "app.page",
        PluginExtensionSurfaceKind::Page,
        PluginCapability::UiPage,
        true,
        true,
    ),
];

#[must_use]
pub(crate) fn definitions() -> Vec<PluginExtensionTargetDefinition> {
    TARGETS
        .iter()
        .map(
            |(target_id, surface_kind, required_capability, contextual, accepts_forms)| {
                PluginExtensionTargetDefinition {
                    target_id: PluginExtensionTargetId::parse(*target_id)
                        .expect("built-in plugin target id must be valid"),
                    surface_kind: *surface_kind,
                    required_capability: *required_capability,
                    contextual: *contextual,
                    accepts_forms: *accepts_forms,
                }
            },
        )
        .collect()
}

#[must_use]
pub(crate) fn find(target_id: &PluginExtensionTargetId) -> Option<PluginExtensionTargetDefinition> {
    definitions()
        .into_iter()
        .find(|definition| definition.target_id == *target_id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::definitions;

    #[test]
    fn target_registry_is_unique_and_excludes_security_surfaces() {
        let definitions = definitions();
        let ids = definitions
            .iter()
            .map(|definition| definition.target_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), definitions.len());
        assert!(ids.contains("app.header.actions"));
        assert!(ids.iter().all(|id| {
            !id.contains("vault")
                && !id.contains("credential")
                && !id.contains("authentication")
                && !id.contains("hostKey")
                && !id.contains("permissionApproval")
        }));
    }

    #[test]
    fn app_page_is_contextual_so_core_can_bind_plugin_and_page_identity() {
        let page = definitions()
            .into_iter()
            .find(|definition| definition.target_id.as_str() == "app.page")
            .expect("app.page target");
        assert!(page.contextual);
        assert!(page.accepts_forms);
    }

    #[test]
    fn terminal_tools_and_floating_mounts_are_contextual_panels() {
        let definitions = definitions();
        for id in [
            "terminal.tools",
            "terminal.header",
            "terminal.footer",
            "terminal.floating",
            "app.content.floating",
        ] {
            let target = definitions
                .iter()
                .find(|target| target.target_id.as_str() == id)
                .expect("plugin mount");
            assert!(target.contextual && target.accepts_forms);
            assert_eq!(
                target.required_capability,
                norishell_core_api::PluginCapability::UiPanel
            );
        }
    }

    #[test]
    fn ordinary_content_targets_are_contextual_host_rendered_ui_panels() {
        let definitions = definitions();
        for id in [
            "app.content.before",
            "app.content.after",
            "app.content.sidebar",
            "app.content.footer",
        ] {
            let target = definitions
                .iter()
                .find(|definition| definition.target_id.as_str() == id)
                .expect("ordinary content target");
            assert!(target.contextual);
            assert!(target.accepts_forms);
            assert_eq!(
                target.required_capability,
                norishell_core_api::PluginCapability::UiPanel
            );
            assert!(matches!(
                target.surface_kind,
                norishell_core_api::PluginExtensionSurfaceKind::Inline
                    | norishell_core_api::PluginExtensionSurfaceKind::Sidebar
            ));
        }
    }
}
