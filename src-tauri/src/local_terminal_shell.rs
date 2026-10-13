//! Maps the user's local-terminal shell preference onto a concrete launch and
//! lists the shells this machine can offer.
//!
//! The preference is one portable string (see
//! `norishell_core_api::parse_local_terminal_shell`). Nothing in it is trusted
//! as an executable until it is resolved here: fixed ids only resolve against
//! trusted platform locations, and custom paths must be existing absolute
//! executable files. Resolution happens on the spawn thread, so a shell that
//! disappears after being saved fails that one open with a typed
//! `ShellUnavailable` instead of falling back to another shell.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use norishell_core_api::{
    ApplicationPreferenceGroupId, LocalTerminalShellPreference, parse_local_terminal_shell,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    host_service::HostService,
    local_terminal_platform::{
        LocalPtySpawnError, LocalPtySpawnFailure, LocalShellLaunch, default_shell_path,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum LocalShellSelection {
    /// The platform default account shell.
    #[default]
    Default,
    Cmd,
    WindowsPowerShell,
    PowerShell7,
    GitBash,
    /// A user-chosen executable; validated when the shell starts.
    Path(PathBuf),
}

impl LocalShellSelection {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match parse_local_terminal_shell(value)? {
            LocalTerminalShellPreference::Fixed("default") => Self::Default,
            LocalTerminalShellPreference::Fixed("cmd") => Self::Cmd,
            LocalTerminalShellPreference::Fixed("windowsPowerShell") => Self::WindowsPowerShell,
            LocalTerminalShellPreference::Fixed("powershell7") => Self::PowerShell7,
            LocalTerminalShellPreference::Fixed("gitBash") => Self::GitBash,
            LocalTerminalShellPreference::Fixed(_) => return None,
            LocalTerminalShellPreference::Path(path) => Self::Path(PathBuf::from(path)),
        })
    }

    /// Reads the selection from a stored `application` preference value. A
    /// group that was never saved, or saved before the field existed, selects
    /// the platform default; a present but malformed field is `None`.
    pub(crate) fn from_application_value(value: Option<&Value>) -> Option<Self> {
        match value.and_then(|value| value.get("localTerminalShell")) {
            None => Some(Self::Default),
            Some(shell) => shell.as_str().and_then(Self::parse),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionReadError {
    /// The preference store could not be read; retrying may succeed.
    Unavailable,
    /// The stored value does not follow the current grammar.
    Invalid,
}

/// Reads the saved selection. Callers must fail the open on error instead of
/// guessing a shell, so a preference problem never launches an unintended one.
pub(crate) fn load_selection(
    hosts: &HostService,
) -> Result<LocalShellSelection, SelectionReadError> {
    let snapshot = hosts
        .with_desktop_repository(|repository| {
            repository.get_application_preferences(ApplicationPreferenceGroupId::Application)
        })
        .map_err(|_| SelectionReadError::Unavailable)?;
    LocalShellSelection::from_application_value(snapshot.value.as_ref())
        .ok_or(SelectionReadError::Invalid)
}

fn unavailable(message: impl Into<String>) -> LocalPtySpawnError {
    LocalPtySpawnError::new(
        LocalPtySpawnFailure::ShellUnavailable,
        io::Error::other(message.into()),
    )
}

/// Android always starts its fixed system shell; the preference is a desktop
/// setting and is ignored there.
#[cfg(target_os = "android")]
pub(crate) fn resolve(
    _selection: &LocalShellSelection,
) -> Result<Option<LocalShellLaunch>, LocalPtySpawnError> {
    Ok(None)
}

/// Resolves a selection to a launch, or `None` for the platform default.
#[cfg(not(target_os = "android"))]
pub(crate) fn resolve(
    selection: &LocalShellSelection,
) -> Result<Option<LocalShellLaunch>, LocalPtySpawnError> {
    let launch = match selection {
        LocalShellSelection::Default => return Ok(None),
        LocalShellSelection::Path(path) => {
            validate_executable(path).map_err(|error| unavailable(error.to_string()))?;
            LocalShellLaunch {
                program: path.clone(),
                args: custom_shell_args(),
            }
        }
        fixed => fixed_launch(fixed)?,
    };
    Ok(Some(launch))
}

/// Interactive-mode flag for a user-chosen shell. POSIX shells start
/// interactively because stdin is a PTY, but `-i` keeps behavior explicit and
/// matches how the platform default shell is started.
#[cfg(all(unix, not(target_os = "android")))]
fn custom_shell_args() -> Vec<OsString> {
    vec![OsString::from("-i")]
}

#[cfg(windows)]
fn custom_shell_args() -> Vec<OsString> {
    Vec::new()
}

#[cfg(not(target_os = "android"))]
fn validate_executable(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("shell path is not absolute"));
    }
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("shell path is not a file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::other("shell path is not executable"));
        }
    }
    Ok(())
}

/// The fixed ids name Windows shells and cannot start anywhere else.
#[cfg(all(unix, not(target_os = "android")))]
fn fixed_launch(_selection: &LocalShellSelection) -> Result<LocalShellLaunch, LocalPtySpawnError> {
    Err(unavailable(
        "the selected shell is not available on this platform",
    ))
}

#[cfg(windows)]
fn fixed_launch(selection: &LocalShellSelection) -> Result<LocalShellLaunch, LocalPtySpawnError> {
    let locations = windows_shells::Locations::from_env();
    let launch = match selection {
        LocalShellSelection::Cmd => windows_shells::cmd(&locations),
        LocalShellSelection::WindowsPowerShell => windows_shells::windows_powershell(&locations),
        LocalShellSelection::PowerShell7 => windows_shells::powershell7(&locations),
        LocalShellSelection::GitBash => windows_shells::git_bash(&locations),
        LocalShellSelection::Default | LocalShellSelection::Path(_) => None,
    };
    launch.ok_or_else(|| unavailable("the selected shell is not installed"))
}

/// Windows shell discovery against trusted install locations only. `bash.exe`
/// is deliberately never looked up on `PATH`: `System32\bash.exe` is the WSL
/// launcher, which is out of scope for this selection.
#[cfg(any(windows, test))]
mod windows_shells {
    use std::path::PathBuf;
    #[cfg(windows)]
    use std::{env, ffi::OsString, path::Path};

    #[cfg(windows)]
    use crate::local_terminal_platform::LocalShellLaunch;

    pub(super) struct Locations {
        pub(super) comspec: Option<PathBuf>,
        pub(super) system_root: Option<PathBuf>,
        pub(super) program_files: Vec<PathBuf>,
        pub(super) local_app_data: Option<PathBuf>,
        pub(super) path_dirs: Vec<PathBuf>,
    }

    impl Locations {
        #[cfg(windows)]
        pub(super) fn from_env() -> Self {
            let absolute = |name: &str| {
                env::var_os(name)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
            };
            let mut program_files = Vec::new();
            for name in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
                if let Some(dir) = absolute(name)
                    && !program_files.contains(&dir)
                {
                    program_files.push(dir);
                }
            }
            Self {
                comspec: absolute("COMSPEC"),
                system_root: absolute("SystemRoot"),
                program_files,
                local_app_data: absolute("LOCALAPPDATA"),
                path_dirs: env::var_os("PATH")
                    .map(|value| {
                        env::split_paths(&value)
                            .filter(|dir| dir.is_absolute())
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        }
    }

    #[cfg(windows)]
    fn first_file(candidates: Vec<PathBuf>) -> Option<PathBuf> {
        candidates.into_iter().find(|path| path.is_file())
    }

    pub(super) fn cmd_candidates(locations: &Locations) -> Vec<PathBuf> {
        let mut candidates: Vec<PathBuf> = locations.comspec.iter().cloned().collect();
        if let Some(root) = &locations.system_root {
            candidates.push(root.join("System32").join("cmd.exe"));
        }
        candidates
    }

    pub(super) fn windows_powershell_candidates(locations: &Locations) -> Vec<PathBuf> {
        locations
            .system_root
            .iter()
            .map(|root| {
                root.join("System32")
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe")
            })
            .collect()
    }

    pub(super) fn powershell7_candidates(locations: &Locations) -> Vec<PathBuf> {
        locations
            .program_files
            .iter()
            .map(|dir| dir.join("PowerShell").join("7").join("pwsh.exe"))
            .chain(locations.path_dirs.iter().map(|dir| dir.join("pwsh.exe")))
            .collect()
    }

    pub(super) fn git_bash_candidates(locations: &Locations) -> Vec<PathBuf> {
        let mut candidates: Vec<PathBuf> = locations
            .program_files
            .iter()
            .map(|dir| dir.join("Git").join("bin").join("bash.exe"))
            .collect();
        if let Some(dir) = &locations.local_app_data {
            candidates.push(
                dir.join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
            );
        }
        // A relocated Git for Windows still exposes `<root>\cmd\git.exe` on PATH.
        for dir in &locations.path_dirs {
            if dir
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
                && dir.join("git.exe").is_file()
                && let Some(root) = dir.parent()
            {
                candidates.push(root.join("bin").join("bash.exe"));
            }
        }
        candidates
    }

    #[cfg(windows)]
    fn launch(program: Option<PathBuf>, args: &[&str]) -> Option<LocalShellLaunch> {
        program.map(|program| LocalShellLaunch {
            program,
            args: args.iter().map(OsString::from).collect(),
        })
    }

    #[cfg(windows)]
    pub(super) fn cmd(locations: &Locations) -> Option<LocalShellLaunch> {
        launch(first_file(cmd_candidates(locations)), &[])
    }

    #[cfg(windows)]
    pub(super) fn windows_powershell(locations: &Locations) -> Option<LocalShellLaunch> {
        launch(
            first_file(windows_powershell_candidates(locations)),
            &["-NoLogo"],
        )
    }

    #[cfg(windows)]
    pub(super) fn powershell7(locations: &Locations) -> Option<LocalShellLaunch> {
        launch(first_file(powershell7_candidates(locations)), &["-NoLogo"])
    }

    #[cfg(windows)]
    pub(super) fn git_bash(locations: &Locations) -> Option<LocalShellLaunch> {
        launch(
            first_file(git_bash_candidates(locations)),
            &["--login", "-i"],
        )
    }

    #[cfg(windows)]
    pub(super) fn display_path(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalShellOption {
    /// The value to store in `localTerminalShell`.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalShellCatalog {
    /// File name of the shell the platform starts when nothing is selected.
    pub(crate) default_shell_name: String,
    pub(crate) options: Vec<LocalShellOption>,
}

/// Android has no selectable shell.
#[cfg(target_os = "android")]
fn detected_options() -> Vec<LocalShellOption> {
    Vec::new()
}

#[cfg(windows)]
fn detected_options() -> Vec<LocalShellOption> {
    let locations = windows_shells::Locations::from_env();
    [
        ("cmd", "cmd", windows_shells::cmd(&locations)),
        (
            "windowsPowerShell",
            "Windows PowerShell",
            windows_shells::windows_powershell(&locations),
        ),
        (
            "powershell7",
            "PowerShell 7",
            windows_shells::powershell7(&locations),
        ),
        ("gitBash", "Git Bash", windows_shells::git_bash(&locations)),
    ]
    .into_iter()
    .filter_map(|(id, name, launch)| {
        launch.map(|launch| LocalShellOption {
            id: id.to_owned(),
            name: name.to_owned(),
            path: windows_shells::display_path(&launch.program),
        })
    })
    .collect()
}

#[cfg(all(unix, not(target_os = "android")))]
fn detected_options() -> Vec<LocalShellOption> {
    let text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    parse_etc_shells(&text)
        .into_iter()
        .filter(|path| validate_executable(path).is_ok())
        .filter_map(|path| {
            let path_text = path.to_str()?.to_owned();
            let id = format!("path:{path_text}");
            // Skip entries the stored-value grammar would reject so every
            // offered id round-trips through validation.
            parse_local_terminal_shell(&id)?;
            Some(LocalShellOption {
                name: path.file_name()?.to_string_lossy().into_owned(),
                id,
                path: path_text,
            })
        })
        .collect()
}

/// `/etc/shells` lists login shells the system registered; entries that only
/// exist to refuse logins are not usable terminals.
#[cfg(all(unix, not(target_os = "android")))]
fn parse_etc_shells(text: &str) -> Vec<PathBuf> {
    let mut shells: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = PathBuf::from(line);
        let refuses_login = path
            .file_name()
            .is_some_and(|name| name == "nologin" || name == "false");
        if path.is_absolute() && !refuses_login && !shells.contains(&path) {
            shells.push(path);
        }
    }
    shells
}

pub(crate) fn catalog() -> LocalShellCatalog {
    LocalShellCatalog {
        default_shell_name: default_shell_path()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default(),
        options: detected_options(),
    }
}

/// Lists the shells the Settings page may offer. Pure read; it starts no
/// process and exposes only installed-location facts.
#[tauri::command]
pub(crate) async fn local_terminal_shell_catalog() -> Result<LocalShellCatalog, String> {
    tauri::async_runtime::spawn_blocking(catalog)
        .await
        .map_err(|_| "local-terminal-shell-catalog-unavailable".to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn selection_defaults_when_the_field_was_never_saved() {
        assert_eq!(
            LocalShellSelection::from_application_value(None),
            Some(LocalShellSelection::Default)
        );
        assert_eq!(
            LocalShellSelection::from_application_value(Some(&json!({"locale": "en"}))),
            Some(LocalShellSelection::Default)
        );
    }

    #[test]
    fn selection_parses_fixed_ids_and_paths_and_rejects_everything_else() {
        let parse = |value: serde_json::Value| {
            LocalShellSelection::from_application_value(Some(&json!({"localTerminalShell": value})))
        };
        assert_eq!(parse(json!("cmd")), Some(LocalShellSelection::Cmd));
        assert_eq!(parse(json!("gitBash")), Some(LocalShellSelection::GitBash));
        assert_eq!(
            parse(json!("path:/opt/homebrew/bin/fish")),
            Some(LocalShellSelection::Path(PathBuf::from(
                "/opt/homebrew/bin/fish"
            )))
        );
        for invalid in [json!("zsh"), json!("path:"), json!(3), json!(null)] {
            assert_eq!(parse(invalid), None);
        }
    }

    #[test]
    fn default_selection_defers_to_the_platform() {
        #[cfg(not(target_os = "android"))]
        assert_eq!(resolve(&LocalShellSelection::Default).unwrap(), None);
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn custom_paths_must_be_existing_absolute_executables() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temp dir");
        let executable = directory.path().join("shell");
        std::fs::write(&executable, "#!/bin/sh\n").expect("write shell");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        let plain_file = directory.path().join("data");
        std::fs::write(&plain_file, "x").expect("write data");
        std::fs::set_permissions(&plain_file, std::fs::Permissions::from_mode(0o644))
            .expect("chmod");

        let launch = resolve(&LocalShellSelection::Path(executable.clone()))
            .expect("executable resolves")
            .expect("explicit selection");
        assert_eq!(launch.program, executable);
        assert_eq!(launch.args, vec![OsString::from("-i")]);

        for rejected in [
            plain_file,
            directory.path().to_path_buf(),
            directory.path().join("missing"),
            PathBuf::from("relative/shell"),
        ] {
            let error = resolve(&LocalShellSelection::Path(rejected)).unwrap_err();
            assert_eq!(error.failure, LocalPtySpawnFailure::ShellUnavailable);
        }
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn windows_only_ids_are_unavailable_on_unix() {
        for selection in [
            LocalShellSelection::Cmd,
            LocalShellSelection::WindowsPowerShell,
            LocalShellSelection::PowerShell7,
            LocalShellSelection::GitBash,
        ] {
            let error = resolve(&selection).unwrap_err();
            assert_eq!(error.failure, LocalPtySpawnFailure::ShellUnavailable);
        }
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn etc_shells_keeps_usable_absolute_entries_once() {
        let parsed = parse_etc_shells(
            "# comment\n\n/bin/sh\n  /bin/zsh  \nrelative/bash\n/usr/sbin/nologin\n/bin/false\n/bin/sh\n",
        );
        assert_eq!(
            parsed,
            vec![PathBuf::from("/bin/sh"), PathBuf::from("/bin/zsh")]
        );
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn every_offered_shell_id_round_trips_through_the_preference_grammar() {
        for option in catalog().options {
            assert_eq!(
                LocalShellSelection::parse(&option.id),
                Some(LocalShellSelection::Path(PathBuf::from(&option.path)))
            );
        }
    }

    #[test]
    fn windows_candidates_prefer_trusted_locations_in_order() {
        use windows_shells::*;

        let locations = Locations {
            comspec: Some(PathBuf::from("/win/System32/cmd.exe")),
            system_root: Some(PathBuf::from("/win")),
            program_files: vec![PathBuf::from("/pf"), PathBuf::from("/pf86")],
            local_app_data: Some(PathBuf::from("/lad")),
            path_dirs: vec![PathBuf::from("/tools")],
        };
        assert_eq!(
            cmd_candidates(&locations),
            vec![
                PathBuf::from("/win/System32/cmd.exe"),
                PathBuf::from("/win").join("System32").join("cmd.exe"),
            ]
        );
        assert_eq!(
            windows_powershell_candidates(&locations),
            vec![
                PathBuf::from("/win")
                    .join("System32")
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe")
            ]
        );
        assert_eq!(
            powershell7_candidates(&locations),
            vec![
                PathBuf::from("/pf")
                    .join("PowerShell")
                    .join("7")
                    .join("pwsh.exe"),
                PathBuf::from("/pf86")
                    .join("PowerShell")
                    .join("7")
                    .join("pwsh.exe"),
                PathBuf::from("/tools").join("pwsh.exe"),
            ]
        );
        let git = git_bash_candidates(&locations);
        assert_eq!(
            git,
            vec![
                PathBuf::from("/pf")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
                PathBuf::from("/pf86")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
                PathBuf::from("/lad")
                    .join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
            ]
        );
        // bash.exe is never taken from PATH: that name is the WSL launcher.
        assert!(git.iter().all(|path| !path.starts_with("/tools")));
    }
}

#[cfg(all(test, windows))]
mod windows_spawn;
