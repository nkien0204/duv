//! Hand-offs to the operating system's desktop: the Trash, the file
//! manager and the clipboard.
//!
//! Everything here shells out to what the platform already provides (or
//! the `trash` crate for the Trash), so duv itself stays free of GUI
//! dependencies. `App` holds these as plain function pointers so tests can
//! swap in fakes and never touch the real Trash, file manager or clipboard.

use std::{
    ffi::OsStr,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

/// Moves `path` to the system Trash. On macOS this uses `NSFileManager`
/// rather than asking Finder, so it never triggers an automation
/// permission prompt (at the cost of Finder's "Put Back" option on some
/// systems; items can still be dragged out of the Trash).
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context.delete(path).map_err(|err| err.to_string())
}

/// What the Trash is called on this platform, for messages: "the Trash",
/// "the Recycle Bin", or on Linux and other freedesktop systems "the Trash"
/// with its folder (e.g. `~/.local/share/Trash`), since there's often no
/// desktop showing it.
pub fn trash_name() -> String {
    if cfg!(target_os = "macos") {
        "the Trash".to_string()
    } else if cfg!(windows) {
        "the Recycle Bin".to_string()
    } else {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        freedesktop_trash_name(
            std::env::var_os("XDG_DATA_HOME").as_deref(),
            home.as_deref(),
        )
    }
}

/// The freedesktop home Trash, `$XDG_DATA_HOME/Trash` (default
/// `~/.local/share/Trash`), as "the Trash (<folder>)" with the home folder
/// shown as `~`. Items on other drives may go to that drive's own Trash
/// folder instead; this names the usual one.
fn freedesktop_trash_name(xdg_data_home: Option<&OsStr>, home: Option<&Path>) -> String {
    let data = xdg_data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(".local").join("share")));
    let Some(folder) = data.map(|data| data.join("Trash")) else {
        return "the Trash".to_string();
    };
    let shown = match home.and_then(|home| folder.strip_prefix(home).ok()) {
        Some(relative) => format!("~/{}", relative.display()),
        None => folder.display().to_string(),
    };
    format!("the Trash ({shown})")
}

/// Shows `path` in the platform's file manager: selected in its folder in
/// Finder (macOS) or Explorer (Windows); on other systems, which have no
/// standard way to select an item, its containing folder is opened with
/// `xdg-open`. Returns once the file manager has been launched.
///
/// Fails with an explanation, without trying, when there's no desktop to
/// show it on: over SSH, or on a Linux/BSD machine with no graphical
/// display (e.g. a server).
pub fn reveal(path: &Path) -> Result<(), String> {
    if let Some(reason) = missing_desktop(over_ssh(), has_display()) {
        return Err(format!(
            "there's no desktop here ({reason}). Press y to copy the path instead."
        ));
    }
    let mut command;
    if cfg!(target_os = "macos") {
        command = Command::new("open");
        command.arg("-R").arg(path);
    } else if cfg!(windows) {
        command = Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
    } else {
        command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
    }
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("couldn't run {program}: {err}"))?;
    // Reap it in the background so it doesn't linger as a zombie; its exit
    // status isn't meaningful (Explorer, for one, reports failure anyway).
    thread::spawn(move || child.wait());
    Ok(())
}

/// Copies `text` to the clipboard.
///
/// Uses the platform's clipboard tool (`pbcopy`, PowerShell's
/// `Set-Clipboard`, or `wl-copy`/`xclip`/`xsel`). Over SSH those would
/// copy on the remote machine, so there — and on Linux when none of the
/// tools is installed — it asks the terminal to copy instead with an
/// OSC 52 escape sequence, which most modern terminals support.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    if over_ssh() {
        return copy_with_terminal(text);
    }
    copy_with_system_tool(text)
}

#[cfg(target_os = "macos")]
fn copy_with_system_tool(text: &str) -> Result<(), String> {
    pipe_to(&mut Command::new("pbcopy"), text).map_err(|err| format!("pbcopy: {err}"))
}

#[cfg(windows)]
fn copy_with_system_tool(text: &str) -> Result<(), String> {
    // Passed through an environment variable to avoid any quoting issues;
    // `clip.exe` would mangle non-ASCII paths.
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Set-Clipboard -Value $env:DUV_CLIPBOARD",
        ])
        .env("DUV_CLIPBOARD", text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("powershell: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("powershell Set-Clipboard failed ({status})"))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn copy_with_system_tool(text: &str) -> Result<(), String> {
    let mut tools: Vec<Vec<&str>> = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tools.push(vec!["wl-copy"]);
    }
    tools.push(vec!["xclip", "-selection", "clipboard"]);
    tools.push(vec!["xsel", "--clipboard", "--input"]);

    for tool in &tools {
        let mut command = Command::new(tool[0]);
        command.args(&tool[1..]);
        match pipe_to(&mut command, text) {
            Ok(()) => return Ok(()),
            // Not installed: try the next one.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("{}: {err}", tool[0])),
        }
    }
    // No clipboard tool (e.g. a headless machine): let the terminal copy.
    copy_with_terminal(text)
}

/// Runs `command` with `text` on its standard input and waits for it.
#[cfg_attr(windows, allow(dead_code))]
fn pipe_to(command: &mut Command, text: &str) -> io::Result<()> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("exited with {status}")))
    }
}

/// Why a file manager can't be shown, if it can't: over SSH it would open
/// on the remote machine's screen, and without a display there's nowhere
/// to open it at all.
fn missing_desktop(over_ssh: bool, has_display: bool) -> Option<&'static str> {
    if over_ssh {
        Some("this is an SSH session")
    } else if !has_display {
        Some("no graphical display was found")
    } else {
        None
    }
}

/// Whether a graphical display is available. macOS and Windows always have
/// one for a local user; elsewhere it's an X11 or Wayland session.
fn has_display() -> bool {
    cfg!(any(target_os = "macos", windows))
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Whether duv is running in an SSH session, where the local clipboard
/// tools would copy on the remote machine.
fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// Asks the terminal to put `text` on the clipboard (OSC 52). The TUI
/// draws to stderr, so the sequence goes there too. There's no way to tell
/// whether the terminal supports it; unsupported terminals ignore it.
fn copy_with_terminal(text: &str) -> Result<(), String> {
    let mut stderr = io::stderr();
    stderr
        .write_all(osc52(text).as_bytes())
        .and_then(|()| stderr.flush())
        .map_err(|err| format!("couldn't write to the terminal: {err}"))
}

/// The OSC 52 "set clipboard" escape sequence for `text`.
fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Standard base64 with padding (all OSC 52 needs; not worth a crate).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("/tmp/é".as_bytes()), "L3RtcC/DqQ==");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    // The freedesktop Trash only exists on Unix; elsewhere paths use other
    // separators and this naming is never used.
    #[cfg(unix)]
    #[test]
    fn freedesktop_trash_is_named_with_its_folder() {
        let home = Path::new("/home/kn");
        assert_eq!(
            freedesktop_trash_name(None, Some(home)),
            "the Trash (~/.local/share/Trash)"
        );
        assert_eq!(
            freedesktop_trash_name(Some(OsStr::new("/home/kn/data")), Some(home)),
            "the Trash (~/data/Trash)"
        );
        assert_eq!(
            freedesktop_trash_name(Some(OsStr::new("/srv/xdg")), Some(home)),
            "the Trash (/srv/xdg/Trash)"
        );
        // A relative XDG_DATA_HOME is invalid per the spec and ignored.
        assert_eq!(
            freedesktop_trash_name(Some(OsStr::new("relative")), Some(home)),
            "the Trash (~/.local/share/Trash)"
        );
        assert_eq!(freedesktop_trash_name(None, None), "the Trash");
    }

    #[test]
    fn reveal_needs_a_local_desktop() {
        assert_eq!(missing_desktop(false, true), None);
        assert_eq!(
            missing_desktop(true, true),
            Some("this is an SSH session"),
            "a display on the remote machine doesn't help"
        );
        assert_eq!(
            missing_desktop(false, false),
            Some("no graphical display was found")
        );
    }

    #[test]
    fn osc52_wraps_the_base64_text() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
