//! Shared pieces of ClaudeHUD: the wire protocol between the hook binary and the
//! HUD, Win32 helpers for finding/focusing terminal windows, and the installer
//! that wires the hook into Claude Code's settings.json.

pub mod install;
pub mod protocol;
pub mod win;
