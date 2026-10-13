//! ConPTY smoke tests for the selectable Windows shells.
//!
//! Each test resolves a shell exactly as `local_terminal_open` does, spawns it
//! through the real platform layer and checks that a computed value comes back
//! through the pseudo console. A shell that is not installed on the machine is
//! skipped, so the suite stays usable on bare CI images.

use std::{
    io::Read,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::{LocalShellSelection, resolve};
use crate::local_terminal_platform::LocalPtyProcess;

/// 98765 * 4321. The product never appears in the typed command line, so
/// finding it proves the shell evaluated the expression instead of echoing it.
const EXPECTED_PRODUCT: &str = "426763565";
const OUTPUT_DEADLINE: Duration = Duration::from_secs(25);

fn run_arithmetic(selection: LocalShellSelection, command: &str) -> Option<String> {
    let launch = match resolve(&selection) {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("skipping {selection:?}: {error:?}");
            return None;
        }
    };
    let (_, mut process) =
        LocalPtyProcess::spawn(24, 120, launch.as_ref()).expect("spawn selected shell");
    let mut reader = process.try_clone_reader().expect("clone PTY reader");
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    // Detached on purpose: the read only ends once the process is torn down.
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    process
        .try_clone_writer()
        .expect("clone PTY writer")
        .write_input(format!("{command}\r").as_bytes())
        .expect("write command");

    let deadline = Instant::now() + OUTPUT_DEADLINE;
    let mut output = Vec::new();
    while Instant::now() < deadline {
        if let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(200)) {
            output.extend_from_slice(&chunk);
        }
        if String::from_utf8_lossy(&output).contains(EXPECTED_PRODUCT) {
            break;
        }
    }
    let _ = process.terminate();
    Some(String::from_utf8_lossy(&output).into_owned())
}

fn assert_computes(selection: LocalShellSelection, command: &str) {
    if let Some(text) = run_arithmetic(selection, command) {
        assert!(text.contains(EXPECTED_PRODUCT), "{text}");
    }
}

#[test]
fn cmd_runs_through_conpty() {
    assert_computes(LocalShellSelection::Cmd, "set /a 98765*4321");
}

#[test]
fn windows_powershell_runs_through_conpty() {
    assert_computes(LocalShellSelection::WindowsPowerShell, "98765*4321");
}

#[test]
fn git_bash_runs_through_conpty() {
    assert_computes(LocalShellSelection::GitBash, "echo $((98765*4321))");
}

#[test]
fn custom_path_runs_without_arguments() {
    let comspec = std::env::var_os("COMSPEC").expect("COMSPEC is set on Windows");
    assert_computes(
        LocalShellSelection::Path(PathBuf::from(comspec)),
        "set /a 98765*4321",
    );
}
