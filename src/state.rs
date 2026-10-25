//! Session bookkeeping, fed by hook events.

use std::collections::BTreeSet;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use claudehud::protocol::Decision;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    Working,
    /// Claude asked a question and waits for an answer.
    Waiting,
    Done,
    /// The turn ended on an API error (StopFailure).
    Error,
}

pub struct Pending {
    pub id: u64,
    pub tool: String,
    pub summary: String,
    pub at: Instant,
    pub tx: Sender<Decision>,
}

pub struct Session {
    pub id: String,
    pub name: String,
    pub hwnd: i64,
    /// The Claude Code process, see [`crate::protocol::HookMsg::pid`].
    pub pid: u32,
    pub status: Status,
    /// The last prompt the user submitted.
    pub prompt: String,
    /// The last tool step, e.g. "Bash: cargo test".
    pub step: String,
    /// Final answer, question or error text of the current Done/Waiting/Error state.
    pub message: String,
    /// Files edited during the current/last turn.
    pub files: BTreeSet<String>,
    pub pending: Vec<Pending>,
    pub working_since: Option<Instant>,
    /// How long the last finished turn took.
    pub run_time: Option<Duration>,
    /// When the session last started needing attention (Done/Waiting/Error).
    pub attention_at: Option<Instant>,
    /// The user has seen the current Done/Waiting/Error state.
    pub seen: bool,
    pub last_event: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoteKind {
    Done,
    Waiting,
    /// Carries the pending permission's id.
    Permission(u64),
    Error,
}

/// A notification card waiting to be shown (or showing).
pub struct Note {
    pub id: u64,
    pub session_id: String,
    pub kind: NoteKind,
    pub at: Instant,
    /// Sound and reporting preferences have been applied.
    pub announced: bool,
}

#[derive(Default)]
pub struct State {
    pub sessions: Vec<Session>,
    pub notes: Vec<Note>,
    next_id: u64,
}

impl State {
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn session(&mut self, ev: &Value, hwnd: i64, pid: u32) -> &mut Session {
        let id = ev["session_id"].as_str().unwrap_or("unknown").to_string();
        let idx = match self.sessions.iter().position(|s| s.id == id) {
            Some(i) => i,
            None => {
                let cwd = ev["cwd"].as_str().unwrap_or("");
                let name = cwd
                    .trim_end_matches(['\\', '/'])
                    .rsplit(['\\', '/'])
                    .next()
                    .filter(|n| !n.is_empty())
                    .unwrap_or("claude")
                    .to_string();
                self.sessions.push(Session {
                    id,
                    name,
                    hwnd: 0,
                    pid: 0,
                    status: Status::Idle,
                    prompt: String::new(),
                    step: String::new(),
                    message: String::new(),
                    files: BTreeSet::new(),
                    pending: Vec::new(),
                    working_since: None,
                    run_time: None,
                    attention_at: None,
                    seen: true,
                    last_event: Instant::now(),
                });
                self.sessions.len() - 1
            }
        };
        let s = &mut self.sessions[idx];
        if hwnd != 0 {
            s.hwnd = hwnd;
        }
        if pid != 0 {
            s.pid = pid;
        }
        s.last_event = Instant::now();
        s
    }

    /// Display name, disambiguated when several sessions share a folder.
    pub fn label(&self, s: &Session) -> String {
        let dupes = self.sessions.iter().filter(|o| o.name == s.name).count();
        if dupes > 1 {
            format!("{} · {}", s.name, &s.id[..s.id.len().min(4)])
        } else {
            s.name.clone()
        }
    }

    fn notify(&mut self, session_id: String, kind: NoteKind) {
        // A session has at most one Done/Waiting/Error card; the newest wins.
        if !matches!(kind, NoteKind::Permission(_)) {
            self.drop_notes(&session_id);
        }
        let id = self.next_id();
        self.notes.push(Note { id, session_id, kind, at: Instant::now(), announced: false });
    }

    /// Removes a session's Done/Waiting/Error cards (permission cards stay until answered).
    pub fn drop_notes(&mut self, session_id: &str) {
        self.notes
            .retain(|n| n.session_id != session_id || matches!(n.kind, NoteKind::Permission(_)));
    }

    pub fn add_pending(&mut self, ev: &Value, hwnd: i64, pid: u32, tx: Sender<Decision>) -> u64 {
        let id = self.next_id();
        let tool = ev["tool_name"].as_str().unwrap_or("tool").to_string();
        let summary = summarize_tool(&tool, &ev["tool_input"]);
        let s = self.session(ev, hwnd, pid);
        s.status = Status::Working;
        s.working_since.get_or_insert_with(Instant::now);
        s.step = format!("{tool}: {summary}");
        s.pending.push(Pending { id, tool, summary, at: Instant::now(), tx });
        let sid = s.id.clone();
        self.notify(sid, NoteKind::Permission(id));
        id
    }

    pub fn remove_pending(&mut self, id: u64) {
        for s in &mut self.sessions {
            s.pending.retain(|p| p.id != id);
        }
        self.notes.retain(|n| n.kind != NoteKind::Permission(id));
    }

    pub fn apply(&mut self, ev: &Value, hwnd: i64, pid: u32) {
        let name = ev["hook_event_name"].as_str().unwrap_or("");
        if name == "SessionEnd" {
            let id = ev["session_id"].as_str().unwrap_or("");
            self.sessions.retain(|s| s.id != id);
            self.notes.retain(|n| n.session_id != id);
            return;
        }
        let s = self.session(ev, hwnd, pid);
        let sid = s.id.clone();
        match name {
            "SessionStart" => {
                if let Some(title) = ev["session_title"].as_str().filter(|t| !t.is_empty()) {
                    s.prompt = title.to_string();
                }
            }
            "UserPromptSubmit" => {
                s.status = Status::Working;
                s.seen = true;
                s.working_since = Some(Instant::now());
                s.files.clear();
                s.step.clear();
                s.prompt = one_line(ev["prompt"].as_str().unwrap_or(""));
                self.drop_notes(&sid);
            }
            "PreToolUse" => {
                let was_working = s.status == Status::Working;
                s.status = Status::Working;
                s.seen = true;
                s.working_since.get_or_insert_with(Instant::now);
                let tool = ev["tool_name"].as_str().unwrap_or("tool");
                s.step = format!("{tool}: {}", summarize_tool(tool, &ev["tool_input"]));
                if matches!(tool, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
                    && let Some(path) = ev["tool_input"]["file_path"].as_str().or(ev["tool_input"]["notebook_path"].as_str())
                {
                    s.files.insert(path.to_string());
                }
                if !was_working {
                    self.drop_notes(&sid);
                }
            }
            "Stop" => {
                s.status = Status::Done;
                s.seen = false;
                s.attention_at = Some(Instant::now());
                s.run_time = s.working_since.take().map(|t| t.elapsed());
                let msg = ev["last_assistant_message"].as_str().unwrap_or("");
                s.message = if msg.trim().is_empty() { "Hotovo".into() } else { one_line(msg) };
                self.notify(sid, NoteKind::Done);
            }
            "StopFailure" => {
                s.status = Status::Error;
                s.seen = false;
                s.attention_at = Some(Instant::now());
                s.run_time = s.working_since.take().map(|t| t.elapsed());
                s.message = error_text(ev);
                self.notify(sid, NoteKind::Error);
            }
            "Notification" => {
                let kind = ev["notification_type"].as_str().unwrap_or("");
                let msg = one_line(ev["message"].as_str().unwrap_or(""));
                match kind {
                    // A permission dialog we aren't handling ourselves (e.g. the
                    // user chose "Otevřít"): just surface it.
                    "permission_prompt" if s.pending.is_empty() => {
                        s.status = Status::Waiting;
                        s.seen = false;
                        s.attention_at = Some(Instant::now());
                        s.message = msg;
                    }
                    "idle_prompt" | "agent_needs_input" | "elicitation_dialog" | "elicitation_url_dialog" => {
                        // idle_prompt fires a while after Stop; don't re-alert a finished turn.
                        if kind == "idle_prompt" && matches!(s.status, Status::Done | Status::Error | Status::Idle) {
                            return;
                        }
                        s.status = Status::Waiting;
                        s.seen = false;
                        s.attention_at = Some(Instant::now());
                        s.message = msg;
                        self.notify(sid, NoteKind::Waiting);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn error_text(ev: &Value) -> String {
    let details = ev["error_details"].as_str().map(one_line).filter(|d| !d.is_empty());
    let kind = match ev["error"].as_str().unwrap_or("") {
        "rate_limit" => "Narazil na limit API".to_string(),
        "overloaded" => "API je přetížené".to_string(),
        "authentication_failed" => "Přihlášení selhalo".to_string(),
        "" | "unknown" => "Chyba API".to_string(),
        other => format!("Chyba API: {other}"),
    };
    match details {
        Some(d) => format!("{kind} · {d}"),
        None => kind,
    }
}

pub fn one_line(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.trim_start_matches(['#', '*', '-', ' '])
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect()
}

fn summarize_tool(tool: &str, input: &Value) -> String {
    let field = |k: &str| input[k].as_str().map(one_line);
    let s = match tool {
        "Bash" | "PowerShell" => field("command"),
        "Edit" | "MultiEdit" | "Write" | "Read" | "NotebookEdit" => field("file_path").or(field("notebook_path")),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Grep" | "Glob" => field("pattern"),
        "Agent" | "Task" => field("description"),
        _ => None,
    };
    s.unwrap_or_else(|| one_line(&input.to_string()))
}
