//! Adds/removes ClaudeHUD's hooks in the user-level Claude Code settings.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Whether a hook command points at ClaudeHUD: the standalone
/// `claudehud-hook.exe`, or `ClaudeHUD.exe hook`.
fn is_ours(command: &str) -> bool {
    let c = command.to_lowercase();
    c.contains("claudehud-hook") || (c.contains("claudehud") && c.trim_end().ends_with(" hook"))
}

/// (event, blocking?) — only PermissionRequest waits for the HUD's answer;
/// everything else runs async so Claude Code never waits on us.
const EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", false),
    ("PermissionRequest", true),
    ("Notification", false),
    ("Stop", false),
    ("StopFailure", false),
    ("SessionEnd", false),
];

pub fn settings_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".claude").join("settings.json")
}

fn load(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn save(path: &Path, settings: &Value) -> Result<(), String> {
    if path.exists() {
        let backup = path.with_extension("json.claudehud-backup");
        std::fs::copy(path, &backup).map_err(|e| format!("cannot back up settings: {e}"))?;
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Drops every hook handler pointing at us; returns the hooks object.
fn strip_ours(settings: &mut Value) -> &mut Map<String, Value> {
    if !settings.is_object() {
        *settings = json!({});
    }
    let root = settings.as_object_mut().unwrap();
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else { continue };
        for group in groups.iter_mut() {
            if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                handlers.retain(|h| !h["command"].as_str().is_some_and(is_ours));
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    hooks
}

/// The command line Claude Code runs: `exe`, plus `arg` if given.
pub fn command_for(exe: &Path, arg: Option<&str>) -> String {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    // Forward slashes + no verbatim prefix: works in Git Bash, cmd and PowerShell.
    let path = exe.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    let path = if path.contains(' ') { format!("\"{path}\"") } else { path };
    match arg {
        Some(arg) => format!("{path} {arg}"),
        None => path,
    }
}

/// `settings` with our handlers (and only ours) pointing at `command`.
fn with_hooks(mut settings: Value, command: &str) -> Value {
    let hooks = strip_ours(&mut settings);
    for &(event, blocking) in EVENTS {
        let handler = if blocking {
            json!({ "type": "command", "command": command, "timeout": 86400 })
        } else {
            json!({ "type": "command", "command": command, "async": true, "timeout": 10 })
        };
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        if let Some(groups) = groups.as_array_mut() {
            groups.push(json!({ "hooks": [handler] }));
        }
    }
    settings
}

/// Points the hooks at `command` unless they already are; returns whether
/// settings.json changed. Used by the HUD on every start.
pub fn ensure(command: &str) -> Result<bool, String> {
    let path = settings_path();
    let settings = load(&path)?;
    let wanted = with_hooks(settings.clone(), command);
    if wanted == settings {
        return Ok(false);
    }
    save(&path, &wanted)?;
    Ok(true)
}

pub fn install() -> Result<(), String> {
    let path = settings_path();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = command_for(&exe, None);
    let settings = with_hooks(load(&path)?, &command);
    save(&path, &settings)?;
    println!("Installed ClaudeHUD hooks into {}", path.display());
    println!("Hook command: {command}");
    println!("Restart running Claude Code sessions (or run /hooks) to pick them up.");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let path = settings_path();
    let mut settings = load(&path)?;
    let hooks = strip_ours(&mut settings);
    if hooks.is_empty() {
        settings.as_object_mut().unwrap().remove("hooks");
    }
    save(&path, &settings)?;
    println!("Removed ClaudeHUD hooks from {}", path.display());
    Ok(())
}
