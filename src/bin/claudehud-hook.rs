//! Standalone console build of the hook handler (see `claudehud::hook`), for
//! development. Also: `claudehud-hook install` / `uninstall` / `probe` (shows which
//! terminal window a hook started from here would jump to).

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
        None => claudehud::hook::run(),
    }
}

fn exit_with(result: Result<(), String>) {
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
