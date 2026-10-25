//! Linux: the window runs as the desktop user, so writes go through `bootkiln write
//! --json-progress`, started as root with `pkexec` (polkit asks for the password).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use bootkiln_core::Error;
use bootkiln_core::job::{Note, Phase};
use serde::Deserialize;

use crate::WriteRequest;

/// pkexec's exit codes when the password dialog is dismissed / the user is not allowed.
const PKEXEC_DISMISSED: i32 = 126;
const PKEXEC_NOT_AUTHORIZED: i32 = 127;

#[derive(Deserialize)]
#[serde(untagged)]
enum Line {
    Progress { phase: String, done: u64, total: u64 },
    Finished { note: String },
    Cancelled { cancelled: bool },
}

/// Run `request` in the privileged helper; `progress` as for `Job::run` (an error from it
/// asks the helper to cancel).
pub fn write(request: &WriteRequest, progress: &mut dyn FnMut(Phase, u64, u64) -> Result<(), Error>) -> Result<Note, Error> {
    let helper = Helper::locate()?;
    let mut child = Command::new("pkexec")
        .arg(&helper.path)
        .args(arguments(request)?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| other(format!("starting pkexec (polkit) to get root rights: {e}")))?;
    let mut stdin = child.stdin.take();
    let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));

    let mut outcome = None;
    for line in stdout.lines() {
        let line = line.map_err(Error::Io)?;
        match serde_json::from_str::<Line>(&line) {
            Ok(Line::Progress { phase, done, total }) => {
                let Some(phase) = Phase::ALL.into_iter().find(|p| p.name() == phase) else { continue };
                if progress(phase, done, total).is_err()
                    && let Some(mut input) = stdin.take()
                {
                    let _ = input.write_all(b"cancel\n");
                    // Dropping `input` closes the pipe, which the helper also treats as cancel.
                }
            }
            Ok(Line::Finished { note }) => outcome = Note::ALL.into_iter().find(|n| n.name() == note).map(Ok),
            Ok(Line::Cancelled { cancelled: true }) => outcome = Some(Err(Error::Cancelled)),
            Ok(Line::Cancelled { .. }) | Err(_) => {}
        }
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(Error::Io)?;
    match output.status.code() {
        Some(PKEXEC_DISMISSED) => return Err(Error::Cancelled),
        Some(PKEXEC_NOT_AUTHORIZED) => return Err(other("not authorized to write drives (polkit refused)".into())),
        _ => {}
    }
    if let Some(outcome) = outcome {
        return outcome;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim().trim_start_matches("Error: ");
    Err(other(if message.is_empty() { format!("the write helper failed ({})", output.status) } else { message.to_owned() }))
}

fn arguments(request: &WriteRequest) -> Result<Vec<String>, Error> {
    let image = std::path::absolute(&request.image).map_err(Error::Io)?;
    let mode = match request.mode {
        crate::ModeArg::Auto => "auto",
        crate::ModeArg::Iso => "iso",
        crate::ModeArg::Dd => "dd",
    };
    let mut args = vec![
        "write".to_owned(),
        image.to_string_lossy().into_owned(),
        "--drive".to_owned(),
        request.drive.to_string(),
        "--mode".to_owned(),
        mode.to_owned(),
        "--yes".to_owned(),
        "--json-progress".to_owned(),
    ];
    let windows = &request.windows;
    let flags = [
        (request.force, "--force"),
        (!request.verify, "--no-verify"),
        (windows.skip_hardware_checks, "--win-skip-hw-checks"),
        (windows.skip_online_account, "--win-skip-online-account"),
        (windows.copy_region, "--win-copy-region"),
        (windows.skip_privacy_questions, "--win-skip-privacy"),
        (windows.disable_bitlocker, "--win-no-bitlocker"),
    ];
    args.extend(flags.iter().filter(|(on, _)| *on).map(|(_, flag)| (*flag).to_owned()));
    if let Some(name) = windows.local_account.as_deref().filter(|n| !n.trim().is_empty()) {
        args.extend(["--win-local-account".to_owned(), name.to_owned()]);
    }
    Ok(args)
}

/// The `bootkiln` CLI next to this executable. Inside an AppImage it is copied out first:
/// root cannot read the user's FUSE mount of the AppImage.
struct Helper {
    path: PathBuf,
    temporary: bool,
}

impl Helper {
    fn locate() -> Result<Helper, Error> {
        let exe = std::env::current_exe().map_err(Error::Io)?;
        let bundled = exe.with_file_name("bootkiln");
        if !bundled.is_file() {
            return Err(other(format!("the write helper {} is missing", bundled.display())));
        }
        if std::env::var_os("APPIMAGE").is_none() {
            return Ok(Helper { path: bundled, temporary: false });
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let path = dir.join(format!("bootkiln-helper-{}", std::process::id()));
        std::fs::copy(&bundled, &path).map_err(|e| other(format!("copying the write helper to {}: {e}", path.display())))?;
        Ok(Helper { path, temporary: true })
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn other(message: String) -> Error {
    Error::Io(std::io::Error::other(message))
}
