//! Launch at login, per OS: a LaunchAgent on macOS, the `Run` registry key on
//! Windows, an XDG autostart entry on Linux.

use std::io;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

const ID: &str = "com.proart.app";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// LaunchAgent plist that starts the app in menu-bar mode at login.
pub fn launch_agent_plist(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{ID}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>--menubar</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy())
    )
}

/// XDG autostart entry.
pub fn desktop_entry(exe: &Path) -> String {
    let quoted = format!("\"{}\"", exe.to_string_lossy().replace('"', "\\\""));
    format!(
        "[Desktop Entry]\nType=Application\nName=ProArt\nComment=Control ASUS ProArt monitors\nExec={quoted} --menubar\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(not(windows))]
fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn entry_path() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{ID}.plist"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn entry_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("autostart/proart.desktop")
}

#[cfg(unix)]
pub fn enabled() -> bool {
    entry_path().exists()
}

#[cfg(unix)]
pub fn set_enabled(on: bool, exe: &Path) -> io::Result<()> {
    let path = entry_path();
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(target_os = "macos")]
    let text = launch_agent_plist(exe);
    #[cfg(not(target_os = "macos"))]
    let text = desktop_entry(exe);
    std::fs::write(path, text)
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
pub fn enabled() -> bool {
    std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", "ProArt"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn set_enabled(on: bool, exe: &Path) -> io::Result<()> {
    let mut cmd = std::process::Command::new("reg");
    if on {
        let value = format!("\"{}\" --menubar", exe.display());
        cmd.args(["add", RUN_KEY, "/v", "ProArt", "/t", "REG_SZ", "/d", &value, "/f"]);
    } else {
        cmd.args(["delete", RUN_KEY, "/v", "ProArt", "/f"]);
    }
    let out = cmd.output()?;
    if out.status.success() || !on {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_starts_in_menubar_mode_and_escapes_the_path() {
        let p = launch_agent_plist(Path::new("/Applications/Pro & Art/ProArt"));
        assert!(p.contains("<string>/Applications/Pro &amp; Art/ProArt</string>"));
        assert!(p.contains("--menubar") && p.contains("RunAtLoad"));
    }

    #[test]
    fn desktop_entry_quotes_the_executable() {
        let d = desktop_entry(Path::new("/opt/pro art/ProArt"));
        assert!(d.contains(r#"Exec="/opt/pro art/ProArt" --menubar"#));
    }
}
