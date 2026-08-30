//! "Launch at login" support via a per-user LaunchAgent, since OhMyLock
//! ships as a bare binary (or a lightweight .app wrapper around one) rather
//! than something with an installer that could register a login item through
//! `SMAppService`.

use anyhow::{Context, Result};
use directories::BaseDirs;
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "dev.facelock.FaceLock";

fn plist_path() -> Result<PathBuf> {
    let base = BaseDirs::new().context("could not determine home directory")?;
    Ok(base
        .home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn gui_domain() -> Result<String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to run `id -u`")?;
    let uid = String::from_utf8(output.stdout)
        .context("`id -u` produced non-UTF8 output")?
        .trim()
        .to_string();
    Ok(format!("gui/{uid}"))
}

pub fn is_enabled() -> bool {
    plist_path().map(|p| p.exists()).unwrap_or(false)
}

/// Writes a LaunchAgent plist pointing at the currently running executable
/// and loads it, so OhMyLock starts automatically on the next login.
pub fn enable() -> Result<()> {
    let exec = std::env::current_exe().context("failed to resolve current executable path")?;
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let escaped_exec = exec
        .display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{escaped_exec}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#
    );
    std::fs::write(&path, plist).with_context(|| format!("failed to write {}", path.display()))?;

    let domain = gui_domain()?;
    // Bootout first in case a stale copy from a previous run is already
    // loaded -- bootstrap fails if the label is already registered, and we
    // just rewrote the plist above so any existing registration is stale.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain, &path.display().to_string()])
        .status()
        .context("failed to run `launchctl bootstrap`")?;
    anyhow::ensure!(status.success(), "launchctl bootstrap exited with {status}");
    Ok(())
}

/// Unloads and removes the LaunchAgent, if one is registered.
pub fn disable() -> Result<()> {
    let domain = gui_domain()?;
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();

    let path = plist_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}
