//! Environment and argv shaping for user-facing local shells.
//!
//! The desktop process inherits whatever its launcher handed it, and that is
//! not what a terminal should pass on: launchd gives macOS GUI apps a minimal
//! environment (no `LANG`, `/usr/bin:/bin:/usr/sbin:/sbin` as `PATH`), the
//! AppImage GTK hook overrides toolkit variables for the bundled libraries,
//! and Windows keeps a snapshot from application start. Each platform corrects
//! only those known launcher artefacts here; everything else the user's
//! session provides is passed through unchanged. Android starts from an empty
//! environment instead, because app UIDs inherit loader/runtime state.

#[cfg(unix)]
use std::{path::Path, process::Command};

#[cfg(any(windows, test))]
use std::ffi::OsString;

/// What every desktop local PTY advertises, whatever the launcher set.
#[cfg(unix)]
const LOCAL_TERMINAL_TYPE: &str = "xterm-256color";
const LOCAL_TERMINAL_COLOR_MODE: &str = "truecolor";
const LOCAL_TERMINAL_PROGRAM: &str = "NoriShell";

/// Applies the local terminal environment to a Unix shell command.
///
/// `explicit_shell` is the shell chosen in preferences, which then becomes
/// `$SHELL`; the platform default keeps the session's value.
#[cfg(unix)]
pub(crate) fn apply_unix_environment(
    command: &mut Command,
    shell: &Path,
    home: &Path,
    explicit_shell: bool,
) -> std::io::Result<()> {
    #[cfg(target_os = "android")]
    android::apply(command, home)?;
    #[cfg(not(target_os = "android"))]
    let _ = home;
    #[cfg(target_os = "linux")]
    for edit in appimage::edits(|name| std::env::var_os(name)) {
        edit.apply(command);
    }
    #[cfg(target_os = "macos")]
    macos::apply(command, shell);
    command
        // The desktop process may itself start with TERM=dumb. The child owns
        // a real PTY, so advertise its actual xterm compatibility instead of
        // inheriting that non-terminal hint.
        .env("TERM", LOCAL_TERMINAL_TYPE)
        .env("COLORTERM", LOCAL_TERMINAL_COLOR_MODE)
        .env("TERM_PROGRAM", LOCAL_TERMINAL_PROGRAM)
        // The host may be launched by an automation environment that sets
        // this process-wide opt-out. A user-facing PTY must not pass that
        // host-only rendering decision to interactive CLIs.
        .env_remove("NO_COLOR");
    if explicit_shell {
        command.env("SHELL", shell);
    }
    Ok(())
}

/// One change to the inherited environment.
#[cfg(any(target_os = "linux", all(test, unix)))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum EnvEdit {
    Set(&'static str, std::ffi::OsString),
    Remove(&'static str),
}

#[cfg(target_os = "linux")]
impl EnvEdit {
    fn apply(self, command: &mut Command) {
        match self {
            Self::Set(name, value) => command.env(name, value),
            Self::Remove(name) => command.env_remove(name),
        };
    }
}

/// Undoes the AppImage runtime and `linuxdeploy-plugin-gtk` hook.
///
/// The hook unconditionally exports toolkit variables that point GTK, GIO and
/// gdk-pixbuf at the libraries inside the mounted image, and forces
/// `GDK_BACKEND=x11` and an Adwaita `GTK_THEME`. Passed to a shell, they make
/// every GTK program the user starts from it load the image's modules and
/// themes. The originals of the overwritten variables are gone by the time
/// this process runs, and a normal desktop session sets none of them, so they
/// are removed; `XDG_DATA_DIRS`, which the hook only prepends to, gets its
/// original value back.
#[cfg(any(target_os = "linux", all(test, unix)))]
mod appimage {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    use super::EnvEdit;

    /// Set by the AppImage runtime itself.
    const RUNTIME: &[&str] = &["APPIMAGE", "APPDIR", "ARGV0", "OWD"];
    /// Overwritten by the GTK hook with image-specific values.
    const GTK_HOOK_OVERRIDES: &[&str] = &[
        "GTK_DATA_PREFIX",
        "GTK_THEME",
        "GDK_BACKEND",
        "GSETTINGS_SCHEMA_DIR",
        "GI_TYPELIB_PATH",
        "GTK_EXE_PREFIX",
        "GTK_PATH",
        "GTK_IM_MODULE_FILE",
        "GDK_PIXBUF_MODULE_FILE",
        "GIO_EXTRA_MODULES",
    ];

    pub(super) fn edits(get: impl Fn(&str) -> Option<OsString>) -> Vec<EnvEdit> {
        let Some(appdir) = get("APPDIR").filter(|value| !value.is_empty()) else {
            return Vec::new();
        };
        if get("APPIMAGE").is_none_or(|value| value.is_empty()) {
            return Vec::new();
        }
        let mut edits: Vec<EnvEdit> = RUNTIME
            .iter()
            .chain(GTK_HOOK_OVERRIDES)
            .filter(|name| get(name).is_some())
            .map(|name| EnvEdit::Remove(name))
            .collect();
        if let Some(value) = get("XDG_DATA_DIRS") {
            match restore_xdg_data_dirs(&appdir, &value) {
                Some(original) if !original.is_empty() => {
                    edits.push(EnvEdit::Set("XDG_DATA_DIRS", original));
                }
                Some(_) => edits.push(EnvEdit::Remove("XDG_DATA_DIRS")),
                None => {}
            }
        }
        edits
    }

    /// The hook sets `$APPDIR/usr/share:/usr/share:<original>`. Returns the
    /// original (possibly empty), or `None` when the value does not carry the
    /// hook's prefix and is left alone.
    fn restore_xdg_data_dirs(appdir: &OsStr, value: &OsStr) -> Option<OsString> {
        let mut prefix = appdir.as_bytes().to_vec();
        prefix.extend_from_slice(b"/usr/share:/usr/share:");
        value
            .as_bytes()
            .strip_prefix(prefix.as_slice())
            .map(|original| OsString::from_vec(original.to_vec()))
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;
        use std::ffi::OsString;

        use super::super::EnvEdit;
        use super::edits;

        fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
            let map: HashMap<String, OsString> = pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
                .collect();
            move |name| map.get(name).cloned()
        }

        const APPDIR: &str = "/tmp/.mount_NoriSh1";

        #[test]
        fn outside_an_appimage_nothing_changes() {
            let get = env(&[("GTK_THEME", "Adwaita:dark"), ("XDG_DATA_DIRS", "/a:/b")]);
            assert!(edits(get).is_empty());
            // APPDIR alone (an extracted tree run by hand) is not an AppImage.
            let get = env(&[("APPDIR", APPDIR), ("GTK_THEME", "Adwaita:dark")]);
            assert!(edits(get).is_empty());
        }

        #[test]
        fn hook_variables_are_removed_and_data_dirs_restored() {
            let data_dirs = format!("{APPDIR}/usr/share:/usr/share:/usr/share/ubuntu:/usr/share");
            let pixbuf = format!("{APPDIR}//usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache");
            let get = env(&[
                ("APPDIR", APPDIR),
                ("APPIMAGE", "/home/u/NoriShell.AppImage"),
                ("OWD", "/home/u"),
                ("GTK_THEME", "Adwaita:light"),
                ("GDK_BACKEND", "x11"),
                ("GDK_PIXBUF_MODULE_FILE", &pixbuf),
                ("XDG_DATA_DIRS", &data_dirs),
                ("PATH", "/usr/bin:/bin"),
            ]);
            let edits = edits(get);
            for name in [
                "APPIMAGE",
                "APPDIR",
                "OWD",
                "GTK_THEME",
                "GDK_BACKEND",
                "GDK_PIXBUF_MODULE_FILE",
            ] {
                assert!(edits.contains(&EnvEdit::Remove(name)), "{name} kept");
            }
            assert!(edits.contains(&EnvEdit::Set(
                "XDG_DATA_DIRS",
                OsString::from("/usr/share/ubuntu:/usr/share")
            )));
            assert!(
                !edits
                    .iter()
                    .any(|edit| matches!(edit, EnvEdit::Remove("PATH") | EnvEdit::Set("PATH", _)))
            );
        }

        #[test]
        fn data_dirs_unset_before_the_hook_is_removed() {
            let data_dirs = format!("{APPDIR}/usr/share:/usr/share:");
            let get = env(&[
                ("APPDIR", APPDIR),
                ("APPIMAGE", "/x.AppImage"),
                ("XDG_DATA_DIRS", &data_dirs),
            ]);
            assert!(edits(get).contains(&EnvEdit::Remove("XDG_DATA_DIRS")));
        }

        #[test]
        fn data_dirs_without_the_hook_prefix_are_kept() {
            let get = env(&[
                ("APPDIR", APPDIR),
                ("APPIMAGE", "/x.AppImage"),
                ("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            ]);
            assert!(!edits(get).iter().any(|edit| matches!(
                edit,
                EnvEdit::Set("XDG_DATA_DIRS", _) | EnvEdit::Remove("XDG_DATA_DIRS")
            )));
        }
    }
}

/// macOS: start the shell as a login shell with a UTF-8 locale, as
/// Terminal.app does.
///
/// Without a login shell, `/etc/zprofile` (`path_helper`, which adds
/// `/etc/paths.d`) and `~/.zprofile` (where Homebrew's `shellenv` lives) never
/// run, so a shell inheriting launchd's `PATH` cannot find user-installed
/// tools. A leading `-` in argv[0] is the login convention every POSIX shell
/// honours, so this works for the chosen shell as well as `pw_shell`.
#[cfg(target_os = "macos")]
mod macos {
    use std::{ffi::OsString, os::unix::process::CommandExt, path::Path, process::Command};

    pub(super) fn apply(command: &mut Command, shell: &Path) {
        if let Some(name) = shell.file_name() {
            let mut argv0 = OsString::from("-");
            argv0.push(name);
            command.arg0(argv0);
        }
        let get = |name: &str| std::env::var_os(name);
        if super::needs_locale(get) {
            let locale = super::utf8_locale(system_language_region(), |name| {
                Path::new("/usr/share/locale").join(name).is_dir()
            });
            command.env("LANG", locale);
        }
    }

    fn system_language_region() -> Option<(String, String)> {
        use objc2_foundation::NSLocale;

        let locale = NSLocale::currentLocale();
        let language = locale.languageCode().to_string();
        let region = locale.regionCode()?.to_string();
        Some((language, region))
    }
}

/// True when no locale category that decides the character set is set.
#[cfg(any(target_os = "macos", test))]
fn needs_locale(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .all(|name| get(name).is_none_or(|value| value.is_empty()))
}

/// `<language>_<REGION>.UTF-8` when the system has that locale, else
/// `en_US.UTF-8`; an unknown pair (such as `en_CN`) must not become a locale
/// the C library rejects.
#[cfg(any(target_os = "macos", test))]
fn utf8_locale(
    language_region: Option<(String, String)>,
    installed: impl Fn(&str) -> bool,
) -> String {
    const FALLBACK: &str = "en_US.UTF-8";
    let Some((language, region)) = language_region else {
        return FALLBACK.to_owned();
    };
    let plain = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphabetic());
    if !plain(&language) || !plain(&region) {
        return FALLBACK.to_owned();
    }
    let candidate = format!("{language}_{region}.UTF-8");
    if installed(&candidate) {
        candidate
    } else {
        FALLBACK.to_owned()
    }
}

/// Android: an empty environment plus what the system shell needs.
#[cfg(target_os = "android")]
mod android {
    use std::{os::unix::fs::DirBuilderExt, path::Path, process::Command};

    const ANDROID_PATH: &str = "/product/bin:/apex/com.android.runtime/bin:\
        /apex/com.android.art/bin:/system_ext/bin:/system/bin:/system/xbin:\
        /odm/bin:/vendor/bin:/vendor/xbin";
    /// Platform locations the system shell and its tools need; they are set
    /// by zygote/init for the app process and carry no user data.
    const ANDROID_PASSTHROUGH_ENV: &[&str] = &[
        "ANDROID_DATA",
        "ANDROID_ROOT",
        "ANDROID_ART_ROOT",
        "ANDROID_I18N_ROOT",
        "ANDROID_TZDATA_ROOT",
        "BOOTCLASSPATH",
    ];

    pub(super) fn apply(command: &mut Command, home: &Path) -> std::io::Result<()> {
        let tmp = home.join("tmp");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&tmp)?;
        command
            .env_clear()
            .env("HOME", home)
            .env("TMPDIR", &tmp)
            .env("PATH", ANDROID_PATH)
            .env("SHELL", super::ANDROID_SHELL);
        for name in ANDROID_PASSTHROUGH_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "android")]
pub(crate) const ANDROID_SHELL: &str = "/system/bin/sh";

/// Windows: the environment block for a new ConPTY shell.
///
/// Windows Terminal rebuilds the user environment from the registry for every
/// new tab, so a `PATH` changed by an installer is visible without restarting
/// it. This does the same: the process environment, overlaid with a fresh
/// `CreateEnvironmentBlock` for the current user, so variables only this
/// session has (drive cwd entries, values a parent passed) still reach the
/// shell. If the fresh block cannot be built the inherited snapshot is used,
/// which is what every terminal got before.
#[cfg(windows)]
pub(crate) fn windows_environment_block() -> std::io::Result<Vec<u16>> {
    let fresh = match windows::fresh_user_environment() {
        Ok(fresh) => fresh,
        Err(error) => {
            eprintln!("local terminal: user environment refresh unavailable: {error}");
            Vec::new()
        }
    };
    let merged = merge_windows_environment(std::env::vars_os().collect(), fresh);
    windows::encode_environment_block(&merged)
}

/// Overlays `fresh` on `inherited` with Windows' case-insensitive names, then
/// applies the terminal's own variables. The result is sorted by upper-cased
/// name, the order Windows keeps environment blocks in.
#[cfg(any(windows, test))]
fn merge_windows_environment(
    inherited: Vec<(OsString, OsString)>,
    fresh: Vec<(OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    use std::collections::BTreeMap;

    let key = |name: &OsString| name.to_string_lossy().to_uppercase();
    let mut merged: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for (name, value) in inherited.into_iter().chain(fresh) {
        merged.insert(key(&name), (name, value));
    }
    // Windows Terminal sets neither TERM nor a terminal-specific opt-out, and
    // native console programs misread a Unix TERM; only the colour hints that
    // the ConPTY output really supports are added.
    for (name, value) in [
        ("COLORTERM", LOCAL_TERMINAL_COLOR_MODE),
        ("TERM_PROGRAM", LOCAL_TERMINAL_PROGRAM),
    ] {
        merged.insert(name.to_owned(), (name.into(), value.into()));
    }
    merged.remove("NO_COLOR");
    merged.into_values().collect()
}

#[cfg(windows)]
mod windows {
    use std::{
        ffi::{OsStr, OsString, c_void},
        io,
        os::windows::ffi::{OsStrExt, OsStringExt},
        ptr::null_mut,
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY},
        System::{
            Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };

    /// The current user's environment as the registry defines it now, without
    /// inheriting this process's values.
    pub(super) fn fresh_user_environment() -> io::Result<Vec<(OsString, OsString)>> {
        let mut token: HANDLE = null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle; token receives an
        // owned handle that is closed below.
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
                &raw mut token,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut block: *mut c_void = null_mut();
        // SAFETY: token is a live token handle with the access the call needs;
        // block receives an allocation released by DestroyEnvironmentBlock.
        let created = unsafe { CreateEnvironmentBlock(&raw mut block, token, 0) };
        let created_error = io::Error::last_os_error();
        // SAFETY: token was opened above and is not used afterwards.
        unsafe { CloseHandle(token) };
        if created == 0 {
            return Err(created_error);
        }
        // SAFETY: a successful CreateEnvironmentBlock returns a block of
        // NUL-terminated UTF-16 strings ending with an empty string.
        let entries = unsafe { parse_environment_block(block.cast()) };
        // SAFETY: block came from CreateEnvironmentBlock and is freed once.
        unsafe { DestroyEnvironmentBlock(block) };
        Ok(entries)
    }

    /// # Safety
    /// `block` must point to a double-NUL-terminated UTF-16 environment block.
    unsafe fn parse_environment_block(mut block: *const u16) -> Vec<(OsString, OsString)> {
        let mut entries = Vec::new();
        loop {
            let mut length = 0;
            // SAFETY: the caller guarantees each string is NUL-terminated.
            while unsafe { *block.add(length) } != 0 {
                length += 1;
            }
            if length == 0 {
                return entries;
            }
            // SAFETY: block..block+length was just read as initialized u16s.
            let entry = unsafe { std::slice::from_raw_parts(block, length) };
            // A leading '=' belongs to the name (drive cwd entries).
            if let Some(split) = entry
                .iter()
                .skip(1)
                .position(|&unit| unit == u16::from(b'='))
            {
                let split = split + 1;
                entries.push((
                    OsString::from_wide(&entry[..split]),
                    OsString::from_wide(&entry[split + 1..]),
                ));
            }
            // SAFETY: the terminating NUL of this string is followed by the
            // next string or the block's final NUL.
            block = unsafe { block.add(length + 1) };
        }
    }

    pub(super) fn encode_environment_block(
        entries: &[(OsString, OsString)],
    ) -> io::Result<Vec<u16>> {
        let mut block = Vec::new();
        for (name, value) in entries {
            let name_units: Vec<u16> = name.encode_wide().collect();
            let valid_name = !name_units.is_empty()
                && !name_units[1..].contains(&u16::from(b'='))
                && !name_units.contains(&0);
            let value_units: Vec<u16> = OsStr::new(value).encode_wide().collect();
            if !valid_name || value_units.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "local terminal environment entry is not representable",
                ));
            }
            block.extend(name_units);
            block.push(u16::from(b'='));
            block.extend(value_units);
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{merge_windows_environment, needs_locale, utf8_locale};

    fn pairs(values: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        values
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn locale_is_added_only_when_no_category_is_set() {
        assert!(needs_locale(|_| None));
        assert!(needs_locale(|name| (name == "LANG").then(OsString::new)));
        for set in ["LC_ALL", "LC_CTYPE", "LANG"] {
            assert!(!needs_locale(|name| (name == set).then(|| "C.UTF-8".into())));
        }
    }

    #[test]
    fn locale_follows_the_system_region_when_installed() {
        let installed = |name: &str| name == "zh_CN.UTF-8";
        let pick = |language: &str, region: &str| {
            utf8_locale(Some((language.into(), region.into())), installed)
        };
        assert_eq!(pick("zh", "CN"), "zh_CN.UTF-8");
        assert_eq!(pick("en", "CN"), "en_US.UTF-8");
        assert_eq!(pick("zh", "C/N"), "en_US.UTF-8");
        assert_eq!(utf8_locale(None, installed), "en_US.UTF-8");
    }

    #[test]
    fn fresh_windows_values_replace_inherited_ones_case_insensitively() {
        let merged = merge_windows_environment(
            pairs(&[
                ("Path", r"C:\Windows"),
                ("=C:", r"C:\Users\u"),
                ("NO_COLOR", "1"),
                ("SESSION_ONLY", "kept"),
            ]),
            pairs(&[
                ("PATH", r"C:\Windows;C:\Program Files\Git\cmd"),
                ("NEW_VAR", "x"),
            ]),
        );
        let get = |name: &str| {
            merged
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(
            get("PATH").as_deref(),
            Some(r"C:\Windows;C:\Program Files\Git\cmd")
        );
        assert_eq!(
            merged
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("path"))
                .count(),
            1
        );
        assert_eq!(get("=C:").as_deref(), Some(r"C:\Users\u"));
        assert_eq!(get("SESSION_ONLY").as_deref(), Some("kept"));
        assert_eq!(get("NEW_VAR").as_deref(), Some("x"));
        assert_eq!(get("COLORTERM").as_deref(), Some("truecolor"));
        assert_eq!(get("TERM_PROGRAM").as_deref(), Some("NoriShell"));
        assert_eq!(get("NO_COLOR"), None);
        assert_eq!(get("TERM"), None);
        let keys: Vec<String> = merged
            .iter()
            .map(|(key, _)| key.to_string_lossy().to_uppercase())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
