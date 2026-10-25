//! Claude Code hook handler. Claude Code runs this for every configured hook
//! event with the event JSON on stdin; we forward it to the HUD. For
//! `PermissionRequest` we wait for the user's click and print the decision.
//!
//! If the HUD isn't running we exit silently and Claude Code behaves as usual.
//!
//! Also: `claudehud-hook install` / `uninstall` / `probe` (shows which
//! terminal window a hook started from here would jump to).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use claudehud::protocol::{self, Decision, HookMsg, Reply};
use serde_json::{Value, json};

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("install") => exit_with(claudehud::install::install()),
        Some("uninstall") => exit_with(claudehud::install::uninstall()),
        Some("probe") => {
            let hwnd = claudehud::win::find_terminal_window();
            println!("terminal window: {hwnd:#x} \"{}\"", claudehud::win::window_title(hwnd));
        }
        Some(other) => {
            eprintln!("unknown command `{other}` (expected install | uninstall | probe, or hook JSON on stdin)");
            std::process::exit(1);
        }
        None => run_hook(),
    }
}

fn exit_with(result: Result<(), String>) {
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run_hook() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(event) = serde_json::from_str::<Value>(input.trim_start_matches('\u{feff}')) else { return };
    let is_permission = event["hook_event_name"] == "PermissionRequest";

    let addr = SocketAddr::from(([127, 0, 0, 1], protocol::port()));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return; // HUD not running
    };
    let msg = HookMsg {
        hwnd: claudehud::win::find_terminal_window(),
        event: event.clone(),
    };
    let Ok(mut line) = serde_json::to_string(&msg) else { return };
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() || !is_permission {
        return;
    }

    let mut reply = String::new();
    if BufReader::new(stream).read_line(&mut reply).is_err() {
        return;
    }
    let Ok(reply) = serde_json::from_str::<Reply>(&reply) else { return };
    let decision = match reply.decision {
        Decision::Ask => return, // no output → regular terminal prompt
        Decision::Allow => json!({ "behavior": "allow" }),
        Decision::Always => match event.get("permission_suggestions") {
            Some(s) if s.as_array().is_some_and(|a| !a.is_empty()) => {
                json!({ "behavior": "allow", "updatedPermissions": s })
            }
            _ => json!({ "behavior": "allow" }),
        },
        Decision::Deny => json!({
            "behavior": "deny",
            "message": "The user denied this from ClaudeHUD.",
        }),
    };
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision,
        }
    });
    println!("{out}");
}
