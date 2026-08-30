//! A single-purpose OS notification: telling the user their face just went
//! out of frame and the absence sentinel is about to auto-lock. Shelled out
//! to `osascript` on macOS instead of pulling in a notification crate --
//! this is the only notification the app ever shows, so a dependency (and
//! its own permission/entitlement handling) isn't worth it for one string.

#[cfg(target_os = "macos")]
pub fn show(title: &str, body: &str) {
    let script = format!(
        "display notification {} with title {}",
        applescript_string_literal(body),
        applescript_string_literal(title)
    );
    // Best-effort: a failed notification (osascript missing, notifications
    // disabled in System Settings, etc.) shouldn't affect the absence
    // sentinel's actual lock behavior, only the heads-up about it.
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();
}

#[cfg(not(target_os = "macos"))]
pub fn show(_title: &str, _body: &str) {}

/// Escapes `s` for embedding inside a double-quoted AppleScript string
/// literal (backslash and double-quote are the only characters that need
/// it there).
#[cfg(target_os = "macos")]
fn applescript_string_literal(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
