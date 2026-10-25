//! Localhost TCP server receiving hook events.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use claudehud::protocol::{HookMsg, Reply};

use crate::state::State;

pub fn spawn(listener: TcpListener, state: Arc<Mutex<State>>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = state.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || handle(stream, state, ctx));
        }
    });
}

fn handle(stream: TcpStream, state: Arc<Mutex<State>>, ctx: eframe::egui::Context) {
    let mut line = String::new();
    let Ok(read_half) = stream.try_clone() else { return };
    if BufReader::new(read_half).read_line(&mut line).is_err() {
        return;
    }
    let Ok(msg) = serde_json::from_str::<HookMsg>(&line) else { return };

    if msg.event["hook_event_name"] != "PermissionRequest" {
        state.lock().unwrap().apply(&msg.event, msg.hwnd);
        ctx.request_repaint();
        return;
    }

    let (tx, rx) = mpsc::channel();
    let id = state.lock().unwrap().add_pending(&msg.event, msg.hwnd, tx);
    ctx.request_repaint();

    // Wait for a click, but give up if Claude Code kills the hook (e.g. the
    // user answered in the terminal) or the session goes away.
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(decision) => {
                let mut out = serde_json::to_string(&Reply { decision }).unwrap();
                out.push('\n');
                let mut stream = &stream;
                let _ = stream.write_all(out.as_bytes());
                break;
            }
            Err(RecvTimeoutError::Timeout) if !is_closed(&stream) => {}
            Err(_) => break,
        }
    }
    state.lock().unwrap().remove_pending(id);
    ctx.request_repaint();
}

fn is_closed(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let mut buf = [0u8; 1];
    let closed = match stream.peek(&mut buf) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => e.kind() != ErrorKind::WouldBlock,
    };
    let _ = stream.set_nonblocking(false);
    closed
}
