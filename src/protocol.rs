use serde::{Deserialize, Serialize};

/// Default TCP port the HUD listens on (localhost only).
pub const DEFAULT_PORT: u16 = 47_821;

pub fn port() -> u16 {
    std::env::var("CLAUDEHUD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// One line of JSON sent by the hook binary to the HUD.
#[derive(Serialize, Deserialize, Debug)]
pub struct HookMsg {
    /// Top-level window of the terminal that hosts the Claude Code session (0 = unknown).
    pub hwnd: i64,
    /// The raw hook payload Claude Code passed on stdin.
    pub event: serde_json::Value,
}

/// The HUD's answer to a `PermissionRequest`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    /// Allow and apply Claude Code's suggested permission rules ("don't ask again").
    Always,
    Deny,
    /// Don't decide; fall back to the regular prompt in the terminal.
    Ask,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Reply {
    pub decision: Decision,
}
