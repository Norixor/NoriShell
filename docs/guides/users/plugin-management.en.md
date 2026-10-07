# Manage plugins

The Plugins page is the only place to import a local package, inspect installed packages, change enabled state, upgrade, or uninstall. A local ZIP is copied to Core-private temporary storage before inspection; the original selected path is not stored by the application UI. Core checks the constrained ZIP, manifest, platform and app compatibility, then shows the self-reported identity, version, SHA-256, and requested capabilities before installation.

NoriShell has no online plugin marketplace or automatic update check. To upgrade, explicitly select a newer local ZIP for the same plugin; it follows the same verification and atomic install path. Every imported package version requires a fresh permission review; grants and host scopes from the installed package are not inherited. Disable stops new guest work and removes its contributions. Uninstall offers the package's supported retain-or-delete choice for plugin-private non-secret data.

Package validation and native acceptance are deliberately separate concerns. Current native desktop, Windows, and hardware acceptance remain P19 work; a package preview, successful import validation, or installed state does not close that gate. Read the current limitations and acceptance evidence in [implementation status](../../../README.md#installation-and-quick-start), and see [permissions and recovery](plugin-safety.en.md) before approving a request.

Platform note: Android (preview, emulator-verified only) supports local ZIP import and isolated-process execution; plugins can only read a read-only snapshot of a selected file, with no directory selection or writing. Plugin approvals and isolated UI are not yet verified on Android; see [platform integration](../../modules/platform-integration.md).
