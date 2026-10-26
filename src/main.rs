//! ClaudeHUD — a companion for Claude Code sessions on Windows.
//!
//! A small glass "island" sits in the bottom-right corner, just above the
//! taskbar, summarizing every session at a glance. Hook events arrive over
//! localhost TCP; when a session finishes, fails or needs you, a notification
//! card rises above the island so you can approve right there or jump to the
//! session's terminal. Clicking the island opens the panel with all sessions.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod mascot;
mod server;
mod settings;
mod sound;
mod state;
mod ui;
mod update;

use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use claudehud::protocol::{self, Decision};
use claudehud::win::{self, Hotkey};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, Id, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
    ViewportBuilder, ViewportCommand, ViewportId, pos2, vec2,
};
use mascot::Pose;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use settings::{Collapse, Settings};
use sound::Tone;
use state::{NoteKind, State, Status};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use ui::{ButtonStyle, Weight, font, mono};

/// Distance of the island from the right edge and the taskbar.
const RIGHT_GAP: f32 = 16.0;
const BOTTOM_GAP: f32 = 12.0;
/// Transparent room left/above the content, wide enough for the glass shadow
/// to fade out fully before the window edge.
const PAD: f32 = 40.0;
const PILL_H: f32 = 40.0;
/// Between the island and the card/panel above it.
const STACK_GAP: f32 = 12.0;
const TOAST_W: f32 = 400.0;
const PANEL_W: f32 = 520.0;
const PANEL_MAX_ROWS: usize = 6;
/// The panel closes this long after the cursor last left it.
const PANEL_LINGER: Duration = Duration::from_secs(4);
/// How long after the last keystroke the HUD stays dimmed; long enough to
/// bridge the pauses between words.
const TYPING_LINGER: Duration = Duration::from_millis(2500);
/// Sound throttling: a "done" chime never follows another sound within 30 s
/// (agents finishing in a burst give one chime), anything else within 4 s.
const DONE_SOUND_GAP: Duration = Duration::from_secs(30);
const SOUND_GAP: Duration = Duration::from_secs(4);

const SETTINGS_TITLE: &str = "ClaudeHUD – Nastavení";
const SETTINGS_SIZE: Vec2 = vec2(640.0, 984.0);

fn main() -> eframe::Result {
    // Claude Code runs `ClaudeHUD.exe hook` for every hook event.
    if std::env::args().nth(1).as_deref() == Some("hook") {
        claudehud::hook::run();
        return Ok(());
    }
    update::remove_previous();

    // Binding the port doubles as a single-instance guard. Right after an
    // update the old copy may still hold it for a moment.
    let addr = SocketAddr::from(([127, 0, 0, 1], protocol::port()));
    let Some(listener) = (0..20).find_map(|attempt| {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(250));
        }
        TcpListener::bind(addr).ok()
    }) else {
        return Ok(());
    };
    let state = Arc::new(Mutex::new(State::default()));
    let hooks = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| claudehud::install::ensure(&claudehud::install::command_for(&exe, Some("hook"))));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ClaudeHUD")
            .with_inner_size([1.0, 1.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false),
        ..Default::default()
    };
    eframe::run_native(
        "ClaudeHUD",
        options,
        Box::new(move |cc| {
            ui::install_fonts(&cc.egui_ctx);
            server::spawn(listener, state.clone(), cc.egui_ctx.clone());
            Ok(Box::new(Hud::new(state, hooks, &cc.egui_ctx)))
        }),
    )
}

// ---------------------------------------------------------------------------
// Tray

enum TrayMsg {
    Toggle,
    Open,
    Settings,
    Uninstall,
    Quit,
}

struct Tray {
    icon: Option<TrayIcon>,
    rx: Receiver<TrayMsg>,
    /// What the icon currently shows, to avoid needless updates.
    shown: Option<(Option<Status>, String)>,
}

impl Tray {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let open = MenuItem::new("Zobrazit relace", true, None);
        let prefs = MenuItem::new("Nastavení", true, None);
        let remove = MenuItem::new("Odebrat hooky a ukončit", true, None);
        let quit = MenuItem::new("Ukončit ClaudeHUD", true, None);
        let menu = Menu::new();
        let _ = menu.append(&open);
        let _ = menu.append(&prefs);
        let _ = menu.append(&remove);
        let _ = menu.append(&quit);
        let (open_id, prefs_id, remove_id, quit_id) =
            (open.id().clone(), prefs.id().clone(), remove.id().clone(), quit.id().clone());

        // Handlers must be Send + Sync; mpsc::Sender is only Send.
        let tray_tx = Mutex::new(tx.clone());
        let tray_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = tray_tx.lock().unwrap().send(TrayMsg::Toggle);
                tray_ctx.request_repaint();
            }
        }));
        let menu_tx = Mutex::new(tx);
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let msg = if e.id == open_id {
                TrayMsg::Open
            } else if e.id == prefs_id {
                TrayMsg::Settings
            } else if e.id == remove_id {
                TrayMsg::Uninstall
            } else if e.id == quit_id {
                TrayMsg::Quit
            } else {
                return;
            };
            let _ = menu_tx.lock().unwrap().send(msg);
            menu_ctx.request_repaint();
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("ClaudeHUD")
            .with_icon(tray_bitmap(None))
            .build()
            .ok();
        Self { icon, rx, shown: None }
    }

    fn update(&mut self, summary: Option<Status>, tooltip: String) {
        let next = (summary, tooltip);
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        if let Some(icon) = &self.icon {
            let _ = icon.set_icon(Some(tray_bitmap(next.0)));
            let _ = icon.set_tooltip(Some(&next.1));
        }
        self.shown = Some(next);
    }
}

/// The tray icon: Claude in the pose matching the most urgent state, an amber
/// badge when something waits for you, faded out when there are no sessions.
fn tray_bitmap(status: Option<Status>) -> tray_icon::Icon {
    tray_icon::Icon::from_rgba(tray_rgba(status), TRAY_PX, TRAY_PX).expect("valid icon")
}

const TRAY_PX: u32 = 64;

fn tray_rgba(status: Option<Status>) -> Vec<u8> {
    const N: u32 = TRAY_PX;
    // One scale for every pose (the widest SVG spans 32 units), so Claude
    // keeps his size and only the prop beside him changes.
    const SCALE: f32 = N as f32 / 32.0;
    let pose = match status {
        Some(Status::Working) => Pose::Working,
        Some(Status::Done) => Pose::Done,
        _ => Pose::Idle,
    };
    let m = mascot::render(pose, SCALE);
    let (left, top) = ((N - m.width.min(N)) / 2, (N - m.height.min(N)) / 2);
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    for y in 0..m.height.min(N) {
        for x in 0..m.width.min(N) {
            let src = ((y * m.width + x) * 4) as usize;
            let dst = (((y + top) * N + x + left) * 4) as usize;
            rgba[dst..dst + 4].copy_from_slice(&m.rgba[src..src + 4]);
        }
    }
    let badge = match status {
        None => {
            for px in rgba.chunks_exact_mut(4) {
                let grey = ((px[0] as u32 * 3 + px[1] as u32 * 6 + px[2] as u32) / 10) as u8;
                px[..3].fill(grey);
                px[3] = (px[3] as f32 * 0.55) as u8;
            }
            None
        }
        Some(Status::Waiting) => Some(ui::AMBER),
        Some(Status::Error) => Some(ui::RED),
        _ => None,
    };
    if let Some(color) = badge {
        // Badge on the top-right corner of the body (SVG unit 18,0; origin -4,-2).
        let (cx, cy) = (left as f32 + 22.0 * SCALE, top as f32 + 2.0 * SCALE);
        for y in 0..N {
            for x in 0..N {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let ring = (9.5 - d).clamp(0.0, 1.0);
                if ring <= 0.0 {
                    continue;
                }
                let fill = (7.5 - d).clamp(0.0, 1.0);
                let c = ui::lerp(ui::BADGE_RING, color, fill);
                let i = ((y * N + x) * 4) as usize;
                let under = rgba[i + 3] as f32 / 255.0;
                let a = ring + under * (1.0 - ring);
                for k in 0..3 {
                    let top_c = [c.r(), c.g(), c.b()][k] as f32;
                    rgba[i + k] = ((top_c * ring + rgba[i + k] as f32 * under * (1.0 - ring)) / a.max(1e-3)) as u8;
                }
                rgba[i + 3] = (a * 255.0) as u8;
            }
        }
    }
    rgba
}

// ---------------------------------------------------------------------------
// Snapshot of the sessions for one frame

/// Display state of a session, in panel order (most urgent first).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Permission,
    Waiting,
    Done,
    Error,
    Working,
    Idle,
}

struct View {
    id: String,
    name: String,
    hwnd: i64,
    status: Status,
    kind: Kind,
    prompt: String,
    step: String,
    message: String,
    files: usize,
    run_time: Option<Duration>,
    working_for: Option<Duration>,
    /// How long the current question/permission has been waiting.
    waiting_for: Option<Duration>,
    attention_at: Option<Instant>,
    /// (pending id, tool, summary) of the oldest open permission request.
    pending: Option<(u64, String, String)>,
}

struct NoteView {
    id: u64,
    view: usize,
    kind: NoteKind,
    at: Instant,
}

struct Snapshot {
    views: Vec<View>,
    /// Display order: permission requests first, then newest first.
    notes: Vec<NoteView>,
}

impl Snapshot {
    fn count(&self, kind: Kind) -> usize {
        self.views.iter().filter(|v| v.kind == kind).count()
    }
}

// ---------------------------------------------------------------------------
// The HUD

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Upper {
    None,
    Toast(u64),
    /// ClaudeHUD's own message: hooks installed, update available, ...
    App,
    Panel,
}

/// Everything needed to draw a notification card, kept so a card can finish
/// fading out after its note is gone.
#[derive(Clone)]
struct Card {
    note: u64,
    kind: NoteKind,
    session_id: String,
    name: String,
    hwnd: i64,
    title: String,
    subtitle: String,
    /// "Tool: summary" for permission requests.
    code: String,
    at: Instant,
}

enum Action {
    Decide(u64, Decision, i64),
    /// Focus the session's terminal and mark it seen.
    Switch(String),
    /// Put the card away; the island keeps showing the state.
    Later(String),
    /// Mark seen and put the card away.
    Ignore(String),
    TogglePanel,
    OpenPanel,
    ClosePanel,
    OpenSettings,
    InstallUpdate,
    /// Hide the app card (until the next start or a newer version).
    DismissApp,
    OpenUrl(&'static str),
}

enum Pill {
    Idle,
    Working { n: usize, longest: Duration, avatars: Vec<String> },
    /// Some working, some finished (the design's corner state).
    Mixed { working: usize, done: usize, avatars: Vec<String> },
    Done { label: String, session_id: String },
    Waiting { label: String, since: Duration },
    Error { label: String },
}

struct Hud {
    state: Arc<Mutex<State>>,
    settings: Settings,
    tray: Tray,
    hotkeys: Receiver<Hotkey>,
    ctx: egui::Context,
    hwnd: Option<isize>,
    /// Logical size and physical top-left of the OS window.
    win_size: Vec2,
    win_origin: (i32, i32),
    click_through: Option<bool>,
    panel_open: bool,
    panel_touch: Instant,
    settings_open: bool,
    settings_hwnd: Option<isize>,
    settings_scroll: f32,
    /// The card/panel currently drawn above the island and its fade (0..1).
    upper: Upper,
    upper_t: f32,
    card: Option<Card>,
    /// Visible time of the current card (pauses while hovered or hidden).
    card_for: Option<u64>,
    card_elapsed: f32,
    last_sound: Option<Instant>,
    /// A sound held back while a full-screen app is in front.
    deferred_tone: Option<Tone>,
    key_presses: u32,
    /// Current opacity from typing and from an idle pill, eased by [`fade`].
    dim: f32,
    idle_dim: f32,
    typing_until: Option<Instant>,
    last_frame: Instant,
    update: update::SharedUpdate,
    /// The update version whose card was put away with "Později".
    update_dismissed: Option<String>,
    /// Kept so the card can fade out after it's dismissed.
    last_app_card: Option<AppCard>,
    /// Outcome of wiring the hooks at startup, shown once.
    hooks_notice: Option<Result<(), String>>,
}

impl Hud {
    fn new(state: Arc<Mutex<State>>, hooks: Result<bool, String>, ctx: &egui::Context) -> Self {
        let update = update::SharedUpdate::default();
        update::check_periodically(update.clone(), ctx.clone());
        let (tx, hotkeys) = mpsc::channel();
        let hk_tx = Mutex::new(tx);
        let hk_ctx = ctx.clone();
        win::spawn_hotkeys(move |key| {
            let _ = hk_tx.lock().unwrap().send(key);
            hk_ctx.request_repaint();
        });
        Self {
            state,
            settings: Settings::load(),
            tray: Tray::new(ctx),
            hotkeys,
            ctx: ctx.clone(),
            hwnd: None,
            win_size: Vec2::ZERO,
            win_origin: (0, 0),
            click_through: None,
            panel_open: false,
            panel_touch: Instant::now(),
            settings_open: std::env::args().any(|a| a == "--settings"),
            settings_hwnd: None,
            settings_scroll: 0.0,
            upper: Upper::None,
            upper_t: 0.0,
            card: None,
            card_for: None,
            card_elapsed: 0.0,
            last_sound: None,
            deferred_tone: None,
            key_presses: win::key_presses(),
            dim: 1.0,
            idle_dim: 1.0,
            typing_until: None,
            last_frame: Instant::now(),
            update,
            update_dismissed: None,
            last_app_card: None,
            hooks_notice: match hooks {
                Ok(true) => Some(Ok(())),
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            },
        }
    }

    fn setup_window(&mut self, frame: &eframe::Frame) {
        if self.hwnd.is_none()
            && let Ok(handle) = frame.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            win::make_overlay(h.hwnd.get());
            self.hwnd = Some(h.hwnd.get());
        }
    }

    fn ppp(ctx: &egui::Context) -> f32 {
        ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0)
    }

    /// Resizes the OS window to `size` (points), anchored to the bottom-right
    /// corner of the work area.
    fn fit_window(&mut self, ctx: &egui::Context, size: Vec2) {
        let Some(hwnd) = self.hwnd else { return };
        let size = vec2(size.x.ceil().max(1.0), size.y.ceil().max(1.0));
        if size != self.win_size {
            let ppp = Self::ppp(ctx);
            let px = |v: f32| (v * ppp).round() as i32;
            self.win_origin = win::place_bottom_right(hwnd, px(size.x), px(size.y));
            self.win_size = size;
        }
    }

    /// The global cursor in window points.
    fn cursor_local(&self, ctx: &egui::Context) -> Pos2 {
        let ppp = Self::ppp(ctx);
        let (x, y) = win::cursor_pos();
        pos2((x - self.win_origin.0) as f32 / ppp, (y - self.win_origin.1) as f32 / ppp)
    }

    /// A key pressed anywhere recently (see [`win::key_presses`]).
    fn track_typing(&mut self) -> bool {
        let presses = win::key_presses();
        if presses != self.key_presses {
            self.key_presses = presses;
            self.typing_until = Some(Instant::now() + TYPING_LINGER);
        }
        self.typing_until.is_some_and(|t| Instant::now() < t)
    }

    fn handle_inputs(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let mut actions = Vec::new();
        while let Ok(msg) = self.tray.rx.try_recv() {
            match msg {
                TrayMsg::Toggle => actions.push(Action::TogglePanel),
                TrayMsg::Open => actions.push(Action::OpenPanel),
                TrayMsg::Settings => actions.push(Action::OpenSettings),
                TrayMsg::Uninstall => {
                    let _ = claudehud::install::uninstall();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                TrayMsg::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
        while let Ok(key) = self.hotkeys.try_recv() {
            match key {
                Hotkey::TogglePanel => actions.push(Action::TogglePanel),
                Hotkey::SwitchToDone => {
                    let st = self.state.lock().unwrap();
                    // The most recently finished session, unseen ones first.
                    let target = st
                        .sessions
                        .iter()
                        .filter(|s| s.status == Status::Done)
                        .max_by_key(|s| (!s.seen, s.attention_at))
                        .map(|s| s.id.clone());
                    if let Some(id) = target {
                        actions.push(Action::Switch(id));
                    }
                }
            }
        }
        actions
    }

    /// Prunes dead sessions, applies reporting preferences to new notes and
    /// snapshots everything for drawing. Returns the tones the new notes ask for.
    fn collect(&mut self, fg: i64) -> (Snapshot, Vec<(Tone, i64, bool)>) {
        let mut st = self.state.lock().unwrap();
        st.sessions.retain(|s| {
            let terminal_gone = s.hwnd != 0 && !win::window_exists(s.hwnd);
            let stale = s.pending.is_empty() && s.last_event.elapsed() > Duration::from_secs(6 * 3600);
            !terminal_gone && !stale
        });
        let alive: Vec<String> = st.sessions.iter().map(|s| s.id.clone()).collect();
        st.notes.retain(|n| alive.contains(&n.session_id));

        // A terminal window shared by several sessions (tabs) can't tell us
        // which one you're looking at.
        let unique_hwnd = |st: &State, hwnd: i64| hwnd != 0 && st.sessions.iter().filter(|s| s.hwnd == hwnd).count() == 1;

        // Looking at a finished session's terminal counts as having seen it.
        let watched: Vec<usize> = (0..st.sessions.len())
            .filter(|&i| {
                let s = &st.sessions[i];
                !s.seen
                    && matches!(s.status, Status::Done | Status::Error)
                    && s.hwnd == fg
                    && unique_hwnd(&st, s.hwnd)
                    && s.attention_at.is_some_and(|t| t.elapsed() > Duration::from_secs(2))
            })
            .collect();
        for i in watched {
            st.sessions[i].seen = true;
        }

        // Announce new notes.
        let mut tones = Vec::new();
        let mut i = 0;
        while i < st.notes.len() {
            if st.notes[i].announced {
                i += 1;
                continue;
            }
            let (kind, sid) = (st.notes[i].kind, st.notes[i].session_id.clone());
            let Some(s) = st.sessions.iter().position(|s| s.id == sid) else {
                st.notes.remove(i);
                continue;
            };
            let wanted = match kind {
                NoteKind::Done => self.settings.notify_done,
                NoteKind::Waiting | NoteKind::Permission(_) => self.settings.notify_waiting,
                NoteKind::Error => self.settings.notify_error,
            };
            if !wanted {
                if let NoteKind::Permission(pid) = kind {
                    // Not reporting: fall back to the prompt in the terminal.
                    let sess = &mut st.sessions[s];
                    if let Some(k) = sess.pending.iter().position(|p| p.id == pid) {
                        let _ = sess.pending.remove(k).tx.send(Decision::Ask);
                    }
                }
                st.notes.remove(i);
                continue;
            }
            st.notes[i].announced = true;
            let tone = match kind {
                NoteKind::Done => Tone::Done,
                NoteKind::Waiting | NoteKind::Permission(_) => Tone::Waiting,
                NoteKind::Error => Tone::Error,
            };
            let hwnd = st.sessions[s].hwnd;
            tones.push((tone, hwnd, unique_hwnd(&st, hwnd)));
            i += 1;
        }

        let views: Vec<View> = st
            .sessions
            .iter()
            .map(|s| {
                let pending = s.pending.first();
                let kind = match s.status {
                    _ if pending.is_some() => Kind::Permission,
                    Status::Waiting => Kind::Waiting,
                    Status::Done if !s.seen => Kind::Done,
                    Status::Error if !s.seen => Kind::Error,
                    Status::Working => Kind::Working,
                    _ => Kind::Idle,
                };
                View {
                    id: s.id.clone(),
                    name: st.label(s),
                    hwnd: s.hwnd,
                    status: s.status,
                    kind,
                    prompt: s.prompt.clone(),
                    step: s.step.clone(),
                    message: s.message.clone(),
                    files: s.files.len(),
                    run_time: s.run_time,
                    working_for: s.working_since.map(|t| t.elapsed()),
                    waiting_for: match pending {
                        Some(p) => Some(p.at.elapsed()),
                        None if s.status == Status::Waiting => s.attention_at.map(|t| t.elapsed()),
                        None => None,
                    },
                    attention_at: s.attention_at,
                    pending: pending.map(|p| (p.id, p.tool.clone(), p.summary.clone())),
                }
            })
            .collect();
        let mut notes: Vec<NoteView> = st
            .notes
            .iter()
            .filter(|n| n.announced)
            .filter_map(|n| {
                let view = views.iter().position(|v| v.id == n.session_id)?;
                Some(NoteView { id: n.id, view, kind: n.kind, at: n.at })
            })
            .collect();
        // Permissions first, oldest first (answer them in order); then newest first.
        notes.sort_by(|a, b| {
            let (pa, pb) = (matches!(a.kind, NoteKind::Permission(_)), matches!(b.kind, NoteKind::Permission(_)));
            pb.cmp(&pa).then_with(|| if pa { a.at.cmp(&b.at) } else { b.at.cmp(&a.at) })
        });
        (Snapshot { views, notes }, tones)
    }

    /// Plays the most urgent of the new notes' tones, unless it would be noise.
    fn play_sounds(&mut self, mut tones: Vec<(Tone, i64, bool)>, fg: i64, hidden: bool) {
        if hidden {
            if let Some(&(tone, ..)) = tones.iter().min_by_key(|t| urgency(t.0)) {
                self.deferred_tone = Some(self.deferred_tone.map_or(tone, |d| if urgency(tone) < urgency(d) { tone } else { d }));
            }
            return;
        }
        if let Some(tone) = self.deferred_tone.take() {
            tones.push((tone, 0, false));
        }
        // You're already looking at the session that wants you.
        tones.retain(|&(_, hwnd, unique)| !(unique && hwnd == fg));
        let Some(&(tone, ..)) = tones.iter().min_by_key(|t| urgency(t.0)) else { return };
        let gap = if tone == Tone::Done { DONE_SOUND_GAP } else { SOUND_GAP };
        if self.last_sound.is_some_and(|t| t.elapsed() < gap) {
            return;
        }
        sound::play(self.settings.sound, tone);
        self.last_sound = Some(Instant::now());
    }

    fn apply(&mut self, action: Action) {
        let mut st = self.state.lock().unwrap();
        match action {
            Action::Decide(id, decision, hwnd) => {
                let mut terminal = (hwnd, 0);
                for s in &mut st.sessions {
                    if let Some(i) = s.pending.iter().position(|p| p.id == id) {
                        let p = s.pending.remove(i);
                        let _ = p.tx.send(decision);
                        s.status = Status::Working;
                        terminal = (s.hwnd, s.pid);
                    }
                }
                st.notes.retain(|n| n.kind != NoteKind::Permission(id));
                drop(st);
                if decision == Decision::Ask {
                    win::focus_session(terminal.0, terminal.1);
                }
            }
            Action::Switch(sid) => {
                let terminal = st.sessions.iter_mut().find(|s| s.id == sid).map(|s| {
                    s.seen = true;
                    (s.hwnd, s.pid)
                });
                st.drop_notes(&sid);
                drop(st);
                if let Some((hwnd, pid)) = terminal {
                    win::focus_session(hwnd, pid);
                }
                self.panel_open = false;
            }
            Action::Later(sid) => st.drop_notes(&sid),
            Action::Ignore(sid) => {
                if let Some(s) = st.sessions.iter_mut().find(|s| s.id == sid) {
                    s.seen = true;
                }
                st.drop_notes(&sid);
            }
            Action::TogglePanel => {
                self.panel_open = !self.panel_open;
                self.panel_touch = Instant::now();
            }
            Action::OpenPanel => {
                self.panel_open = true;
                self.panel_touch = Instant::now();
            }
            Action::ClosePanel => self.panel_open = false,
            Action::OpenSettings => self.settings_open = true,
            Action::InstallUpdate => {
                drop(st);
                let status = self.update.lock().unwrap().clone();
                if let update::Status::Available(release) | update::Status::Failed(_, release) = status {
                    update::install_in_background(self.update.clone(), release, self.ctx.clone());
                }
            }
            Action::DismissApp => {
                drop(st);
                if self.hooks_notice.take().is_none() {
                    let status = self.update.lock().unwrap().clone();
                    match status {
                        update::Status::Available(r) => self.update_dismissed = Some(r.version),
                        update::Status::Failed(_, r) => {
                            self.update_dismissed = Some(r.version.clone());
                            *self.update.lock().unwrap() = update::Status::Available(r);
                        }
                        _ => {}
                    }
                }
            }
            Action::OpenUrl(url) => self.ctx.open_url(egui::OpenUrl::new_tab(url)),
        }
    }

    /// Builds the card for the note at the top of the stack.
    fn card_for_note(snap: &Snapshot, n: &NoteView) -> Card {
        let v = &snap.views[n.view];
        let (title, subtitle, code) = match n.kind {
            NoteKind::Done => {
                let mut parts = Vec::new();
                if !v.message.is_empty() {
                    parts.push(v.message.clone());
                }
                if v.files > 0 {
                    parts.push(format!("{} {}", v.files, plural(v.files, "soubor", "soubory", "souborů")));
                }
                if let Some(d) = v.run_time {
                    parts.push(long_duration(d));
                }
                (format!("{} dokončil práci", v.name), parts.join(" · "), String::new())
            }
            NoteKind::Waiting => (format!("{} čeká na tebe", v.name), v.message.clone(), String::new()),
            NoteKind::Error => (format!("{} narazil na chybu", v.name), v.message.clone(), String::new()),
            NoteKind::Permission(pid) => {
                let (tool, summary) = match &v.pending {
                    Some((id, tool, summary)) if *id == pid => (tool.clone(), summary.clone()),
                    _ => (String::new(), String::new()),
                };
                (format!("{} čeká na tvoje rozhodnutí", v.name), permission_subtitle(&tool), format!("{tool}: {summary}"))
            }
        };
        Card {
            note: n.id,
            kind: n.kind,
            session_id: v.id.clone(),
            name: v.name.clone(),
            hwnd: v.hwnd,
            title,
            subtitle,
            code,
            at: n.at,
        }
    }
}

/// ClaudeHUD's own card, if it has something to say.
#[derive(Clone)]
enum AppCard {
    Hooks(Result<(), String>),
    Update(update::Status),
}

impl Hud {
    fn app_card(&self) -> Option<AppCard> {
        if let Some(notice) = &self.hooks_notice {
            return Some(AppCard::Hooks(notice.clone()));
        }
        let status = self.update.lock().unwrap().clone();
        match &status {
            update::Status::Available(r) if self.update_dismissed.as_deref() == Some(r.version.as_str()) => None,
            update::Status::Available(_)
            | update::Status::Installing
            | update::Status::Restarting
            | update::Status::Failed(..) => Some(AppCard::Update(status)),
            _ => None,
        }
    }
}

fn urgency(tone: Tone) -> u8 {
    match tone {
        Tone::Waiting => 0,
        Tone::Error => 1,
        Tone::Done => 2,
    }
}

fn plural<'a>(n: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    match n {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

/// "6:12", "1:02:03".
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "45 s", "6 min 12 s", "1 h 5 min".
fn long_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, s / 60 % 60),
    }
}

fn ago(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    match s {
        0..60 => "teď".into(),
        60..3600 => format!("před {} min", s / 60),
        _ => format!("před {} h", s / 3600),
    }
}

fn permission_subtitle(tool: &str) -> String {
    match tool {
        "Bash" | "PowerShell" => "Chce spustit příkaz v terminálu".into(),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => "Chce upravit soubor".into(),
        "Read" => "Chce přečíst soubor".into(),
        "WebFetch" => "Chce otevřít web".into(),
        "WebSearch" => "Chce hledat na webu".into(),
        "Agent" | "Task" => "Chce spustit subagenta".into(),
        "" => "Chce použít nástroj".into(),
        other => format!("Chce použít {other}"),
    }
}

/// The island's content for the current sessions.
fn pill_state(snap: &Snapshot) -> Pill {
    let waiting: Vec<&View> = snap.views.iter().filter(|v| matches!(v.kind, Kind::Permission | Kind::Waiting)).collect();
    let done: Vec<&View> = snap.views.iter().filter(|v| v.kind == Kind::Done).collect();
    let working: Vec<&View> = snap.views.iter().filter(|v| v.kind == Kind::Working).collect();
    let errors: Vec<&View> = snap.views.iter().filter(|v| v.kind == Kind::Error).collect();
    let avatars = |list: &[&View]| list.iter().take(3).map(|v| v.name.clone()).collect::<Vec<_>>();

    if !waiting.is_empty() {
        let label = match waiting.len() {
            1 => format!("{} čeká", waiting[0].name),
            n => format!("{n} {}", plural(n, "čeká", "čekají", "čeká")),
        };
        let since = waiting.iter().filter_map(|v| v.waiting_for).max().unwrap_or_default();
        return Pill::Waiting { label, since };
    }
    if !done.is_empty() && !working.is_empty() {
        let both: Vec<&View> = done.iter().chain(working.iter()).copied().collect();
        return Pill::Mixed { working: working.len(), done: done.len(), avatars: avatars(&both) };
    }
    if !done.is_empty() {
        let latest = done.iter().max_by_key(|v| v.attention_at).unwrap();
        let label = match done.len() {
            1 => format!("{} hotovo", latest.name),
            n => format!("{n} hotovo"),
        };
        return Pill::Done { label, session_id: latest.id.clone() };
    }
    if !errors.is_empty() {
        let label = match errors.len() {
            1 => format!("{} selhal", errors[0].name),
            n => format!("{n} {}", plural(n, "selhal", "selhaly", "selhalo")),
        };
        return Pill::Error { label };
    }
    if !working.is_empty() {
        let longest = working.iter().filter_map(|v| v.working_for).max().unwrap_or_default();
        return Pill::Working { n: working.len(), longest, avatars: avatars(&working) };
    }
    Pill::Idle
}

fn working_label(n: usize) -> String {
    format!("{n} {}", plural(n, "pracuje", "pracují", "pracuje"))
}

// ---------------------------------------------------------------------------
// Island

enum Item {
    Spinner,
    Text(String, egui::FontId, Color32),
    Divider,
    Dot(Color32),
    Pulse,
    Check,
    Bang,
    Avatars(Vec<String>),
    Button(String),
}

impl Item {
    fn width(&self, p: &Painter) -> f32 {
        match self {
            Item::Spinner | Item::Check | Item::Bang => 18.0,
            Item::Text(s, f, _) => ui::text_width(p, s, f.clone()),
            Item::Divider => 1.0,
            Item::Dot(_) => 8.0,
            Item::Pulse => 10.0,
            Item::Avatars(a) => 4.0 + 28.0 + 20.0 * (a.len().max(1) - 1) as f32,
            Item::Button(s) => ui::text_width(p, s, font(12.0, Weight::Bold)) + 24.0,
        }
    }
}

struct PillLayout {
    items: Vec<Item>,
    pad_left: f32,
    pad_right: f32,
    gap: f32,
    border: Option<Color32>,
    idle: bool,
}

fn pill_layout(pill: &Pill) -> PillLayout {
    let label = |s: String| Item::Text(s, font(14.0, Weight::Semibold), ui::TEXT);
    let (items, pad_left, border) = match pill {
        Pill::Idle => (Vec::new(), 16.0, None),
        Pill::Working { n, longest, avatars } => (
            vec![
                Item::Spinner,
                label(working_label(*n)),
                Item::Text(clock(*longest), mono(12.0), ui::TEXT_3),
                Item::Avatars(avatars.clone()),
            ],
            12.0,
            None,
        ),
        Pill::Mixed { working, done, avatars } => (
            vec![
                Item::Spinner,
                label(working_label(*working)),
                Item::Divider,
                Item::Dot(ui::GREEN),
                label(format!("{done} hotovo")),
                Item::Avatars(avatars.clone()),
            ],
            12.0,
            None,
        ),
        Pill::Done { label: l, .. } => (
            vec![Item::Check, label(l.clone()), Item::Button("Přepnout".into())],
            14.0,
            Some(Color32::from_rgba_unmultiplied(48, 209, 88, 115)),
        ),
        Pill::Waiting { label: l, since } => (
            vec![Item::Pulse, label(l.clone()), Item::Text(clock(*since), mono(12.0), ui::AMBER_TEXT)],
            14.0,
            Some(Color32::from_rgba_unmultiplied(255, 159, 10, 128)),
        ),
        Pill::Error { label: l } => {
            (vec![Item::Bang, label(l.clone())], 12.0, Some(Color32::from_rgba_unmultiplied(255, 69, 58, 140)))
        }
    };
    let pad_right = match items.last() {
        Some(Item::Avatars(_) | Item::Button(_)) => 6.0,
        _ => 16.0,
    };
    PillLayout { items, pad_left, pad_right, gap: 10.0, border, idle: matches!(pill, Pill::Idle) }
}

fn pill_width(p: &Painter, l: &PillLayout) -> f32 {
    if l.idle {
        return 16.0 + 3.0 * 6.0 + 2.0 * 6.0 + 16.0;
    }
    let items: f32 = l.items.iter().map(|i| i.width(p)).sum();
    l.pad_left + items + l.gap * (l.items.len().saturating_sub(1)) as f32 + l.pad_right
}

fn draw_pill(ui: &egui::Ui, rect: Rect, l: &PillLayout, pill: &Pill, time: f64, actions: &mut Vec<Action>) {
    let resp = ui.interact(rect, Id::new("pill"), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let p = ui.painter();
    ui::bar(p, rect, l.border);
    let cy = rect.center().y;
    if l.idle {
        for i in 0..3 {
            p.circle_filled(pos2(rect.left() + 16.0 + 3.0 + i as f32 * 12.0, cy), 3.0, Color32::from_white_alpha(115));
        }
    } else {
        let mut x = rect.left() + l.pad_left;
        for item in &l.items {
            let w = item.width(p);
            let c = pos2(x + w / 2.0, cy);
            match item {
                Item::Spinner => ui::spinner(p, c, 18.0, 2.5, ui::BLUE, time),
                Item::Text(s, f, color) => {
                    p.text(pos2(x, cy), Align2::LEFT_CENTER, s, f.clone(), *color);
                }
                Item::Divider => {
                    p.line_segment([pos2(c.x, cy - 8.0), pos2(c.x, cy + 8.0)], Stroke::new(1.0, Color32::from_white_alpha(51)));
                }
                Item::Dot(color) => {
                    p.circle_filled(c, 4.0, *color);
                }
                Item::Pulse => ui::pulse_dot(p, c, 10.0, time, false),
                Item::Check => ui::badge_check(p, c, 18.0, false),
                Item::Bang => ui::badge_bang(p, c, 18.0, false),
                Item::Avatars(names) => {
                    // Each avatar overlaps the one before it, as in the design.
                    for (i, name) in names.iter().enumerate() {
                        let r = Rect::from_min_size(pos2(x + 4.0 + 20.0 * i as f32, cy - 14.0), vec2(28.0, 28.0));
                        ui::avatar(p, r, 9.0, name, 11.0, true);
                    }
                }
                Item::Button(label) => {
                    let r = Rect::from_min_size(pos2(x, cy - 14.0), vec2(w, 28.0));
                    if ui::button(ui, r, Id::new("pill-switch"), label, 12.0, ButtonStyle::Solid(ui::GREEN, ui::GREEN_INK))
                        && let Pill::Done { session_id, .. } = pill
                    {
                        actions.push(Action::Switch(session_id.clone()));
                        x += w + l.gap;
                        continue;
                    }
                }
            }
            x += w + l.gap;
        }
    }
    let on_button = matches!(l.items.last(), Some(Item::Button(_)))
        && resp.interact_pointer_pos().is_some_and(|q| q.x > rect.right() - l.items.last().unwrap().width(p) - 6.0);
    if resp.clicked() && !on_button {
        actions.push(Action::TogglePanel);
    }
}

// ---------------------------------------------------------------------------
// Notification card

fn card_height(kind: NoteKind) -> f32 {
    if matches!(kind, NoteKind::Permission(_)) { 180.0 } else { 130.0 }
}

/// Height of the "+ N starší oznámení" line under a card.
const OLDER_H: f32 = 8.0 + 17.0;

fn draw_card(ui: &egui::Ui, rect: Rect, card: &Card, older: usize, drain: Option<f32>, time: f64, actions: &mut Vec<Action>) {
    let p = ui.painter();
    // Older notifications peek out above the card.
    for (k, (inset, lift, opacity)) in [(24.0, 20.0, 0.55), (12.0, 10.0, 0.8)].into_iter().enumerate() {
        if older > 1 - k {
            let ghost = Rect::from_min_max(pos2(rect.left() + inset, rect.top() - lift), pos2(rect.right() - inset, rect.top() - lift + 60.0));
            let mut gp = p.clone();
            gp.multiply_opacity(opacity);
            ui::glass(&gp, ghost, 26.0, None);
        }
    }
    ui::glass(p, rect, 28.0, None);
    // Swallow clicks on the card's background.
    let _ = ui.interact(rect, Id::new(("card", card.note)), Sense::click());

    let permission = matches!(card.kind, NoteKind::Permission(_));
    let pad = if permission { 16.0 } else { 14.0 };
    let av = Rect::from_min_size(rect.min + vec2(pad, pad), vec2(44.0, 44.0));
    ui::avatar(p, av, 13.0, &card.name, 15.0, false);
    match card.kind {
        NoteKind::Done => ui::badge_check(p, av.max - vec2(6.0, 6.0), 20.0, true),
        NoteKind::Error => ui::badge_bang(p, av.max - vec2(6.0, 6.0), 20.0, true),
        NoteKind::Waiting | NoteKind::Permission(_) => ui::pulse_dot(p, av.max - vec2(5.0, 5.0), 16.0, time, true),
    }

    let tx = av.right() + 12.0;
    let right = rect.right() - pad;
    let when = ago(card.at);
    let when_w = ui::text_width(p, &when, font(12.0, Weight::Regular));
    let title_y = av.top() + 12.5;
    let sub_y = av.top() + 32.5;
    ui::text(p, pos2(right, title_y + 1.0), Align2::RIGHT_CENTER, &when, font(12.0, Weight::Regular), ui::TEXT_3, 100.0);
    ui::text(p, pos2(tx, title_y), Align2::LEFT_CENTER, &card.title, font(15.0, Weight::Semibold), ui::TEXT, right - tx - when_w - 10.0);
    ui::text(p, pos2(tx, sub_y), Align2::LEFT_CENTER, &card.subtitle, font(13.0, Weight::Regular), ui::TEXT_2, right - tx);

    let id = Id::new(("card", card.note));
    if permission {
        let code = Rect::from_min_max(pos2(rect.left() + pad, av.bottom() + 12.0), pos2(right, av.bottom() + 48.0));
        p.rect_filled(code, 14.0, Color32::from_black_alpha(71));
        p.rect_stroke(code, 14.0, Stroke::new(1.0, ui::LINE_SUBTLE), StrokeKind::Inside);
        ui::text(p, pos2(code.left() + 12.0, code.center().y), Align2::LEFT_CENTER, &card.code, mono(12.5), ui::CODE, code.width() - 24.0);

        let NoteKind::Permission(pid) = card.kind else { unreachable!() };
        let by = code.bottom() + 12.0;
        let mut x = right;
        let mut btn = |label: &str, extra: f32, style: ButtonStyle| {
            let w = ui::text_width(p, label, font(14.0, Weight::Bold)) + extra;
            let r = Rect::from_min_size(pos2(x - w, by), vec2(w, 44.0));
            x -= w + 8.0;
            ui::button(ui, r, id.with(label), label, 14.0, style)
        };
        if btn("Povolit", 44.0, ButtonStyle::Solid(ui::AMBER, ui::AMBER_INK)) {
            actions.push(Action::Decide(pid, Decision::Allow, card.hwnd));
        }
        if btn("Otevřít", 36.0, ButtonStyle::Glass) {
            actions.push(Action::Decide(pid, Decision::Ask, card.hwnd));
        }
        if btn("Zamítnout", 36.0, ButtonStyle::Glass) {
            actions.push(Action::Decide(pid, Decision::Deny, card.hwnd));
        }
    } else {
        let (primary, color, ink, secondary) = match card.kind {
            NoteKind::Done => ("Přepnout", ui::GREEN, ui::GREEN_INK, "Později"),
            NoteKind::Waiting => ("Odpovědět", ui::AMBER, ui::AMBER_INK, "Později"),
            _ => ("Zobrazit", ui::RED, ui::RED_INK, "Ignorovat"),
        };
        let row = Rect::from_min_max(pos2(tx, av.bottom() + 10.0), pos2(right, av.bottom() + 54.0));
        let half = (row.width() - 8.0) / 2.0;
        let a = Rect::from_min_size(row.min, vec2(half, 44.0));
        let b = Rect::from_min_size(row.min + vec2(half + 8.0, 0.0), vec2(half, 44.0));
        if ui::button(ui, a, id.with("primary"), primary, 14.0, ButtonStyle::Solid(color, ink)) {
            actions.push(Action::Switch(card.session_id.clone()));
        }
        if ui::button(ui, b, id.with("secondary"), secondary, 14.0, ButtonStyle::Glass) {
            actions.push(if card.kind == NoteKind::Error {
                Action::Ignore(card.session_id.clone())
            } else {
                Action::Later(card.session_id.clone())
            });
        }
        if let Some(frac) = drain {
            let track = Rect::from_min_max(pos2(rect.left() + 28.0, rect.bottom() - 9.0), pos2(rect.right() - 28.0, rect.bottom() - 6.0));
            p.rect_filled(track, 2.0, ui::LINE_SUBTLE);
            let fill = Rect::from_min_size(track.min, vec2(track.width() * frac.clamp(0.0, 1.0), track.height()));
            p.rect_filled(fill, 2.0, color.gamma_multiply(0.7));
        }
    }

    if older > 0 {
        let label = format!("+ {older} starší oznámení");
        let pos = pos2(rect.right() - 12.0, rect.bottom() + 8.0);
        let r = ui::text(p, pos, Align2::RIGHT_TOP, &label, font(12.0, Weight::Regular), ui::TEXT_3, 300.0);
        let resp = ui.interact(r.expand(4.0), id.with("older"), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            actions.push(Action::OpenPanel);
        }
    }
}

fn draw_app_card(ui: &egui::Ui, rect: Rect, card: &AppCard, time: f64, actions: &mut Vec<Action>) {
    let p = ui.painter();
    ui::glass(p, rect, 28.0, None);
    let _ = ui.interact(rect, Id::new("app-card"), Sense::click());

    let (title, subtitle, color) = match card {
        AppCard::Hooks(Ok(())) => (
            "ClaudeHUD je připojený".to_string(),
            "Hooky jsou v Claude Code · restartuj běžící relace".to_string(),
            ui::GREEN,
        ),
        AppCard::Hooks(Err(e)) => ("Hooky se nepodařilo zapsat".to_string(), e.clone(), ui::RED),
        AppCard::Update(status) => match status {
            update::Status::Available(r) => (
                format!("Je tu ClaudeHUD {}", r.version),
                format!("Máš {} · stáhne se, ověří a restartuje", update::CURRENT),
                ui::BLUE,
            ),
            update::Status::Installing => ("Aktualizuji…".to_string(), "Stahuji a ověřuji kontrolní součet".to_string(), ui::BLUE),
            update::Status::Restarting => ("Spouštím novou verzi…".to_string(), "Za chvilku jsem zpátky".to_string(), ui::BLUE),
            update::Status::Failed(e, _) => ("Aktualizace se nepovedla".to_string(), e.clone(), ui::RED),
            _ => (String::new(), String::new(), ui::BLUE),
        },
    };

    // The icon tile: check, "!" or a download arrow.
    let av = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(44.0, 44.0));
    p.rect_filled(av, 13.0, color);
    let ink = if color == ui::GREEN {
        ui::GREEN_INK
    } else if color == ui::RED {
        ui::RED_INK
    } else {
        ui::TEXT
    };
    let busy = matches!(card, AppCard::Update(update::Status::Installing | update::Status::Restarting));
    match card {
        _ if busy => ui::spinner(p, av.center(), 22.0, 3.0, Color32::WHITE, time),
        AppCard::Hooks(Ok(())) => ui::check(p, av.center(), 22.0, ink),
        AppCard::Hooks(Err(_)) | AppCard::Update(update::Status::Failed(..)) => {
            p.text(av.center(), Align2::CENTER_CENTER, "!", font(22.0, Weight::Heavy), ink);
        }
        _ => ui::arrow_down(p, av.center(), 22.0, ink),
    }

    let tx = av.right() + 12.0;
    let right = rect.right() - 14.0;
    ui::text(p, pos2(tx, av.top() + 12.5), Align2::LEFT_CENTER, &title, font(15.0, Weight::Semibold), ui::TEXT, right - tx);
    ui::text(p, pos2(tx, av.top() + 32.5), Align2::LEFT_CENTER, &subtitle, font(13.0, Weight::Regular), ui::TEXT_2, right - tx);

    let row = Rect::from_min_max(pos2(tx, av.bottom() + 10.0), pos2(right, av.bottom() + 54.0));
    let half = (row.width() - 8.0) / 2.0;
    let a = Rect::from_min_size(row.min, vec2(half, 44.0));
    let b = Rect::from_min_size(row.min + vec2(half + 8.0, 0.0), vec2(half, 44.0));
    let id = Id::new("app-card");
    match card {
        AppCard::Hooks(_) => {
            if ui::button(ui, row, id.with("ok"), "Rozumím", 14.0, ButtonStyle::Glass) {
                actions.push(Action::DismissApp);
            }
        }
        AppCard::Update(update::Status::Available(_)) => {
            if ui::button(ui, a, id.with("go"), "Aktualizovat", 14.0, ButtonStyle::Solid(ui::BLUE, ui::TEXT)) {
                actions.push(Action::InstallUpdate);
            }
            if ui::button(ui, b, id.with("later"), "Později", 14.0, ButtonStyle::Glass) {
                actions.push(Action::DismissApp);
            }
        }
        AppCard::Update(update::Status::Failed(..)) => {
            if ui::button(ui, a, id.with("web"), "Stáhnout z webu", 14.0, ButtonStyle::Solid(ui::RED, ui::RED_INK)) {
                actions.push(Action::OpenUrl(update::RELEASES_PAGE));
            }
            if ui::button(ui, b, id.with("close"), "Zavřít", 14.0, ButtonStyle::Glass) {
                actions.push(Action::DismissApp);
            }
        }
        _ => {
            // Installing: a sliding bar instead of buttons.
            let track = Rect::from_min_size(pos2(row.left(), row.center().y - 2.0), vec2(row.width(), 4.0));
            p.rect_filled(track, 2.0, ui::LINE_SUBTLE);
            let w = track.width() * 0.3;
            let x = track.left() + (track.width() - w) * (0.5 + 0.5 * (time * 2.2).sin() as f32);
            p.rect_filled(Rect::from_min_size(pos2(x, track.top()), vec2(w, 4.0)), 2.0, ui::BLUE);
        }
    }
}

// ---------------------------------------------------------------------------
// Panel

const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 10.0;

fn panel_rows(snap: &Snapshot) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..snap.views.len()).collect();
    order.sort_by_key(|&i| snap.views[i].kind);
    let overflow = order.len().saturating_sub(PANEL_MAX_ROWS);
    order.truncate(PANEL_MAX_ROWS);
    (order, overflow)
}

fn panel_height(rows: usize) -> f32 {
    14.0 + 50.0 + ROW_GAP + rows.max(1) as f32 * (ROW_H + ROW_GAP) + 22.0 + 14.0
}

fn draw_panel(ui: &egui::Ui, rect: Rect, snap: &Snapshot, settings: &Settings, time: f64, actions: &mut Vec<Action>) {
    let p = ui.painter();
    ui::glass(p, rect, 32.0, None);
    let _ = ui.interact(rect, Id::new("panel"), Sense::click());
    let inner = rect.shrink(14.0);

    // Header
    let hy = inner.top() + 2.0 + 22.0;
    let title = ui::text(p, pos2(inner.left() + 8.0, hy), Align2::LEFT_CENTER, "Claude Code", font(17.0, Weight::Bold), ui::TEXT, 200.0);
    let n = snap.views.len();
    let count = format!("{n} {}", plural(n, "relace", "relace", "relací"));
    ui::text(p, pos2(title.right() + 8.0, hy + 1.5), Align2::LEFT_CENTER, &count, font(13.0, Weight::Regular), ui::TEXT_3, 200.0);
    let collapse = Rect::from_center_size(pos2(inner.right() - 4.0 - 22.0, hy), vec2(44.0, 44.0));
    let gear = collapse.translate(vec2(-54.0, 0.0));
    if ui::icon_button(ui, gear, Id::new("panel-gear"), |p, c| ui::gear(p, c, 18.0, ui::TEXT)) {
        actions.push(Action::OpenSettings);
    }
    if ui::icon_button(ui, collapse, Id::new("panel-collapse"), |p, c| ui::chevron_down(p, c, 16.0, ui::TEXT)) {
        actions.push(Action::ClosePanel);
    }

    // Rows
    let (rows, overflow) = panel_rows(snap);
    let mut y = inner.top() + 50.0 + ROW_GAP;
    if rows.is_empty() {
        let r = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), ROW_H));
        p.rect_filled(r, 22.0, ui::FILL_SUBTLE);
        p.text(r.center(), Align2::CENTER_CENTER, "Žádná relace Claude Code neběží", font(13.0, Weight::Regular), ui::TEXT_3);
    }
    for &i in &rows {
        let v = &snap.views[i];
        let r = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), ROW_H));
        y += ROW_H + ROW_GAP;
        let id = Id::new(("row", &v.id));
        let row_resp = ui.interact(r, id, Sense::click());
        if row_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        let (fill, border) = match v.kind {
            Kind::Done => (Color32::from_rgba_unmultiplied(48, 209, 88, 31), Color32::from_rgba_unmultiplied(48, 209, 88, 71)),
            Kind::Permission | Kind::Waiting => {
                (Color32::from_rgba_unmultiplied(255, 159, 10, 31), Color32::from_rgba_unmultiplied(255, 159, 10, 77))
            }
            Kind::Error => (Color32::from_rgba_unmultiplied(255, 69, 58, 31), Color32::from_rgba_unmultiplied(255, 69, 58, 77)),
            _ => (Color32::from_white_alpha(15), ui::LINE_SUBTLE),
        };
        let fill = if row_resp.hovered() { ui::lerp(fill, Color32::from_white_alpha(40), 0.3) } else { fill };
        p.rect_filled(r, 22.0, fill);
        p.rect_stroke(r, 22.0, Stroke::new(1.0, border), StrokeKind::Inside);

        let av = Rect::from_min_size(r.min + vec2(12.0, 12.0), vec2(40.0, 40.0));
        ui::avatar(p, av, 12.0, &v.name, 14.0, false);

        let (label, style) = match v.kind {
            Kind::Done => ("Přepnout", ButtonStyle::Solid(ui::GREEN, ui::GREEN_INK)),
            Kind::Permission | Kind::Waiting => ("Odpovědět", ButtonStyle::Solid(ui::AMBER, ui::AMBER_INK)),
            Kind::Error => ("Zobrazit", ButtonStyle::Solid(ui::RED, ui::RED_INK)),
            _ => ("Otevřít", ButtonStyle::Glass),
        };
        let bw = ui::text_width(p, label, font(13.0, Weight::Bold)) + 32.0;
        let br = Rect::from_min_size(pos2(r.right() - 12.0 - bw, r.top() + 10.0), vec2(bw, 44.0));
        let clicked = ui::button(ui, br, id.with("btn"), label, 13.0, style);
        if clicked || row_resp.clicked() {
            // A permission is answered on its card, which shows once the panel closes.
            actions.push(if v.kind == Kind::Permission { Action::ClosePanel } else { Action::Switch(v.id.clone()) });
        }

        let tx = av.right() + 12.0;
        let max_w = br.left() - 12.0 - tx;
        let ny = r.top() + 22.5;
        let name = ui::text(p, pos2(tx, ny), Align2::LEFT_CENTER, &v.name, font(15.0, Weight::Semibold), ui::TEXT, max_w * 0.6);
        let mut x = name.right() + 8.0;
        let (tag, color) = match v.kind {
            Kind::Permission => (format!("ČEKÁ NA SCHVÁLENÍ · {}", clock(v.waiting_for.unwrap_or_default())), ui::AMBER),
            Kind::Waiting => (format!("ČEKÁ NA VSTUP · {}", clock(v.waiting_for.unwrap_or_default())), ui::AMBER),
            Kind::Done => (format!("HOTOVO · {}", clock(v.run_time.unwrap_or_default())), ui::GREEN),
            Kind::Error => ("CHYBA".to_string(), ui::RED),
            Kind::Working => (format!("PRACUJE · {}", clock(v.working_for.unwrap_or_default())), ui::BLUE_TAG),
            Kind::Idle => match v.status {
                Status::Done => (format!("HOTOVO · {}", clock(v.run_time.unwrap_or_default())), ui::TEXT_3),
                Status::Error => ("CHYBA".to_string(), ui::TEXT_3),
                _ => ("V KLIDU".to_string(), ui::TEXT_3),
            },
        };
        match v.kind {
            Kind::Working => {
                ui::spinner(p, pos2(x + 6.0, ny), 12.0, 2.0, ui::BLUE_TAG, time);
                x += 20.0;
            }
            Kind::Permission | Kind::Waiting => {
                ui::pulse_dot(p, pos2(x + 3.5, ny), 7.0, time, false);
                x += 15.0;
            }
            _ => {}
        }
        ui::text(p, pos2(x, ny + 0.5), Align2::LEFT_CENTER, &tag, font(11.0, Weight::Bold), color, (tx + max_w - x).max(0.0));

        let dy = r.top() + 44.0;
        let detail_w = max_w;
        match v.kind {
            Kind::Permission => {
                let (tool, summary) = v.pending.as_ref().map(|(_, t, s)| (t.as_str(), s.as_str())).unwrap_or_default();
                ui::text(p, pos2(tx, dy), Align2::LEFT_CENTER, &format!("{tool}: {summary}"), mono(11.5), ui::CODE, detail_w);
            }
            Kind::Working if settings.tool_steps && !v.step.is_empty() => {
                ui::text(p, pos2(tx, dy), Align2::LEFT_CENTER, &v.step, mono(11.5), ui::TEXT_3, detail_w);
            }
            Kind::Working => {
                let s = if v.prompt.is_empty() { &v.step } else { &v.prompt };
                ui::text(p, pos2(tx, dy), Align2::LEFT_CENTER, s, font(12.5, Weight::Regular), ui::TEXT_2, detail_w);
            }
            _ => {
                let s = if v.message.is_empty() { &v.prompt } else { &v.message };
                ui::text(p, pos2(tx, dy), Align2::LEFT_CENTER, s, font(12.5, Weight::Regular), ui::TEXT_2, detail_w);
            }
        }
    }

    // Footer
    let fy = rect.bottom() - 14.0 - 9.0;
    let hint = if overflow > 0 {
        format!("+ {overflow} {}", plural(overflow, "další relace", "další relace", "dalších relací"))
    } else if settings.click_through {
        "Ostrov propouští kliky, dokud nepodržíš Alt".to_string()
    } else {
        "Kliky mimo HUD procházejí do oken pod ním".to_string()
    };
    ui::text(p, pos2(inner.left() + 8.0, fy), Align2::LEFT_CENTER, &hint, font(12.0, Weight::Regular), ui::TEXT_3, inner.width() - 120.0);
    ui::text(p, pos2(inner.right() - 8.0, fy), Align2::RIGHT_CENTER, "Ctrl + Alt + H", font(12.0, Weight::Regular), ui::TEXT_3, 120.0);
}

// ---------------------------------------------------------------------------
// Settings window

enum Row<'a> {
    Toggle(&'a str, Option<&'a str>, Option<Color32>, &'a mut bool),
    /// Label, value text, optional button (label, clicked).
    Action(&'a str, String, Option<(&'a str, &'a mut bool)>),
    Collapse(&'a mut Collapse),
    Sound(&'a mut settings::Sound),
    Keys(&'a str, &'a [&'a str]),
}

impl Row<'_> {
    fn height(&self) -> f32 {
        match self {
            Row::Toggle(_, Some(_), ..) => 61.0,
            Row::Keys(..) => 54.0,
            _ => 56.0,
        }
    }
}

/// Draws a settings section; returns its bottom.
fn section(ui: &egui::Ui, left: f32, width: f32, top: f32, title: &str, rows: Vec<Row>) -> (f32, bool) {
    let p = ui.painter();
    p.text(pos2(left + 16.0, top + 8.0), Align2::LEFT_CENTER, title, font(13.0, Weight::Semibold), ui::TEXT_3);
    let height: f32 = rows.iter().map(Row::height).sum();
    let group = Rect::from_min_size(pos2(left, top + 24.0), vec2(width, height));
    p.rect_filled(group, 22.0, ui::FILL_SUBTLE);
    p.rect_stroke(group, 22.0, Stroke::new(1.0, ui::LINE_SUBTLE), StrokeKind::Inside);
    let mut changed = false;
    let mut y = group.top();
    for (i, row) in rows.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(group.left(), y), vec2(width, row.height()));
        y += row.height();
        if i > 0 {
            p.line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, ui::LINE_SUBTLE));
        }
        let cy = r.center().y;
        let id = Id::new(("setting", title, i));
        match row {
            Row::Toggle(label, sub, dot, value) => {
                let mut x = r.left() + 16.0;
                if let Some(c) = dot {
                    p.circle_filled(pos2(x + 5.0, cy), 5.0, c);
                    x += 22.0;
                }
                match sub {
                    Some(sub) => {
                        p.text(pos2(x, cy - 9.0), Align2::LEFT_CENTER, label, font(15.0, Weight::Medium), ui::TEXT);
                        p.text(pos2(x, cy + 10.0), Align2::LEFT_CENTER, sub, font(12.5, Weight::Regular), ui::TEXT_3);
                    }
                    None => {
                        p.text(pos2(x, cy), Align2::LEFT_CENTER, label, font(15.0, Weight::Medium), ui::TEXT);
                    }
                }
                let sw = Rect::from_min_size(pos2(r.right() - 16.0 - 51.0, cy - 15.5), vec2(51.0, 31.0));
                if ui::switch(ui, sw, id, *value) {
                    *value = !*value;
                    changed = true;
                }
            }
            Row::Collapse(value) => {
                p.text(pos2(r.left() + 16.0, cy), Align2::LEFT_CENTER, "Sbalit oznámení po", font(15.0, Weight::Medium), ui::TEXT);
                let widths: Vec<f32> = Collapse::ALL
                    .iter()
                    .map(|c| ui::text_width(p, c.label(), font(13.0, Weight::Semibold)) + 24.0)
                    .collect();
                let total = widths.iter().sum::<f32>() + 2.0 * 3.0 + 6.0;
                let well = Rect::from_min_size(pos2(r.right() - 16.0 - total, cy - 19.0), vec2(total, 38.0));
                p.rect_filled(well, 12.0, ui::WELL);
                let mut x = well.left() + 3.0;
                for (c, w) in Collapse::ALL.into_iter().zip(widths) {
                    let b = Rect::from_min_size(pos2(x, well.top() + 3.0), vec2(w, 32.0));
                    x += w + 2.0;
                    let resp = ui.interact(b, id.with(c.label()), Sense::click());
                    let selected = *value == c;
                    if selected {
                        p.rect_filled(b, 9.0, Color32::from_white_alpha(51));
                    } else if resp.hovered() {
                        p.rect_filled(b, 9.0, Color32::from_white_alpha(20));
                    }
                    let (weight, color) = if selected { (Weight::Semibold, ui::TEXT) } else { (Weight::Regular, ui::TEXT_2) };
                    p.text(b.center(), Align2::CENTER_CENTER, c.label(), font(13.0, weight), color);
                    if resp.clicked() && !selected {
                        *value = c;
                        changed = true;
                    }
                }
            }
            Row::Sound(value) => {
                let resp = ui.interact(r, id, Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    p.rect_filled(r.shrink(1.0), 0.0, Color32::from_white_alpha(8));
                }
                p.text(pos2(r.left() + 38.0, cy), Align2::LEFT_CENTER, "Zvuk", font(15.0, Weight::Medium), ui::TEXT);
                ui::chevron_right(p, pos2(r.right() - 16.0 - 5.0, cy), 14.0, Color32::from_rgba_unmultiplied(235, 235, 245, 128));
                p.text(pos2(r.right() - 16.0 - 10.0 - 12.0, cy), Align2::RIGHT_CENTER, value.label(), font(14.0, Weight::Regular), ui::TEXT_3);
                if resp.clicked() {
                    *value = value.next();
                    sound::play(*value, Tone::Done);
                    changed = true;
                }
            }
            Row::Action(label, value, button) => {
                p.text(pos2(r.left() + 16.0, cy), Align2::LEFT_CENTER, label, font(15.0, Weight::Medium), ui::TEXT);
                let mut x = r.right() - 16.0;
                if let Some((text, clicked)) = button {
                    let w = ui::text_width(p, text, font(13.0, Weight::Bold)) + 28.0;
                    let b = Rect::from_min_size(pos2(x - w, cy - 16.0), vec2(w, 32.0));
                    if ui::button(ui, b, id.with("btn"), text, 13.0, ButtonStyle::Solid(ui::BLUE, ui::TEXT)) {
                        *clicked = true;
                    }
                    x -= w + 12.0;
                }
                p.text(pos2(x, cy), Align2::RIGHT_CENTER, value, font(14.0, Weight::Regular), ui::TEXT_3);
            }
            Row::Keys(label, keys) => {
                p.text(pos2(r.left() + 16.0, cy), Align2::LEFT_CENTER, label, font(15.0, Weight::Medium), ui::TEXT);
                let mut x = r.right() - 16.0;
                for key in keys.iter().rev() {
                    x -= ui::kbd(p, pos2(x, cy), key) + 4.0;
                }
            }
        }
    }
    (group.bottom(), changed)
}

/// Returns true when the window should close.
fn draw_settings(ui: &mut egui::Ui, s: &mut Settings, scroll: &mut f32, update: &update::Status, install: &mut bool) -> bool {
    let rect = ui.ctx().content_rect();
    ui::glass(ui.painter(), rect, 36.0, None);
    let inner = rect.shrink(24.0);

    // Header: title, close button; the rest of the strip drags the window.
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), 44.0));
    let drag = ui.interact(header, Id::new("settings-drag"), Sense::drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    let p = ui.painter();
    p.text(pos2(inner.left(), header.center().y), Align2::LEFT_CENTER, "Nastavení", font(24.0, Weight::Bold), ui::TEXT);
    let close = Rect::from_min_size(pos2(inner.right() - 44.0, inner.top()), vec2(44.0, 44.0));
    let mut done = ui::icon_button(ui, close, Id::new("settings-close"), |p, c| ui::cross(p, c, 14.0, ui::TEXT));

    // The sections scroll under the header when the screen is too short.
    let body = Rect::from_min_max(pos2(rect.left(), header.bottom() + 22.0), pos2(rect.right(), rect.bottom() - 2.0));
    let content_h = SETTINGS_SIZE.y - 24.0 - 44.0 - 22.0 - 24.0;
    let max_scroll = (content_h - (body.height() - 22.0)).max(0.0);
    if ui.rect_contains_pointer(body) {
        *scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    *scroll = scroll.clamp(0.0, max_scroll);

    let before = s.clone();
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        ui.set_clip_rect(body);
        let (l, w) = (inner.left(), inner.width());
        let y = body.top() - *scroll;
        let (y, _) = section(ui, l, w, y, "Aby nepřekážel", vec![
            Row::Toggle("Proklikávací", Some("Kliky projdou do okna pod ním, dokud nepodržíš Alt"), None, &mut s.click_through),
            Row::Toggle("Ztlumit při psaní", Some("Když píšeš, ostrov zprůhlední na 30 %"), None, &mut s.dim_while_typing),
            Row::Toggle("Skrýt při celé obrazovce", Some("Hry a video — oznámení počkají"), None, &mut s.hide_fullscreen),
            Row::Collapse(&mut s.collapse),
        ]);
        let (y, _) = section(ui, l, w, y + 22.0, "Hlásit", vec![
            Row::Toggle("Hotovo", None, Some(ui::GREEN), &mut s.notify_done),
            Row::Toggle("Čeká na vstup", None, Some(ui::AMBER), &mut s.notify_waiting),
            Row::Toggle("Chyba", None, Some(ui::RED), &mut s.notify_error),
            Row::Toggle("Každý krok nástroje", Some("Jen v panelu, nikdy jako oznámení"), Some(ui::BLUE), &mut s.tool_steps),
            Row::Sound(&mut s.sound),
        ]);
        let (y, _) = section(ui, l, w, y + 22.0, "Zkratky", vec![
            Row::Keys("Přepnout na poslední hotový", &["Ctrl", "Alt", "Mezerník"]),
            Row::Keys("Otevřít panel", &["Ctrl", "Alt", "H"]),
        ]);
        let (value, button) = match update {
            update::Status::Checking => ("Hledám novou verzi…".to_string(), None),
            update::Status::UpToDate => ("Aktuální".to_string(), None),
            update::Status::Available(r) => (format!("Je tu {}", r.version), Some(("Aktualizovat", install))),
            update::Status::Installing | update::Status::Restarting => ("Aktualizuji…".to_string(), None),
            update::Status::Failed(..) => ("Aktualizace selhala".to_string(), Some(("Zkusit znovu", install))),
        };
        section(ui, l, w, y + 22.0, "O aplikaci", vec![Row::Action(
            concat!("ClaudeHUD ", env!("CARGO_PKG_VERSION")),
            value,
            button,
        )]);
    });
    if *s != before {
        s.save();
    }
    done |= ui.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested());
    done
}

impl Hud {
    fn show_settings(&mut self, ctx: &egui::Context) {
        let ppp = Self::ppp(ctx);
        let (l, t, r, b) = win::work_area();
        let center = pos2((l + r) as f32 / 2.0 / ppp, (t + b) as f32 / 2.0 / ppp);
        let size = vec2(SETTINGS_SIZE.x, SETTINGS_SIZE.y.min((b - t) as f32 / ppp - 32.0));
        let builder = ViewportBuilder::default()
            .with_title(SETTINGS_TITLE)
            .with_inner_size(size)
            .with_position(center - size / 2.0)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false);
        let mut close = false;
        let mut install = false;
        let status = self.update.lock().unwrap().clone();
        let (settings, scroll) = (&mut self.settings, &mut self.settings_scroll);
        let id = ViewportId::from_hash_of("settings");
        ctx.show_viewport_immediate(id, builder, |ui, _| {
            close = draw_settings(ui, settings, scroll, &status, &mut install);
        });
        if install {
            self.apply(Action::InstallUpdate);
        }
        if self.settings_hwnd.is_none()
            && let Some(hwnd) = win::own_window(SETTINGS_TITLE)
        {
            win::undecorate(hwnd);
            win::focus_window(hwnd as i64);
            self.settings_hwnd = Some(hwnd);
        }
        if close {
            self.settings_open = false;
            self.settings_hwnd = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Frame

/// Moves an opacity towards `target`: quick to dim, slower to come back, so
/// the island fades instead of blinking.
fn fade(current: f32, target: f32, dt: f32) -> f32 {
    let per_second = if target < current { 0.7 / 0.15 } else { 0.7 / 0.4 };
    let step = per_second * dt;
    if (target - current).abs() <= step { target } else { current + step * (target - current).signum() }
}

/// The most urgent state across sessions, for the tray icon.
fn summarize(snap: &Snapshot) -> Option<Status> {
    let has = |k: Kind| snap.views.iter().any(|v| v.kind == k);
    if snap.views.is_empty() {
        None
    } else if has(Kind::Permission) || has(Kind::Waiting) {
        Some(Status::Waiting)
    } else if has(Kind::Error) {
        Some(Status::Error)
    } else if has(Kind::Done) {
        Some(Status::Done)
    } else if has(Kind::Working) {
        Some(Status::Working)
    } else {
        Some(Status::Idle)
    }
}

impl eframe::App for Hud {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let time = ctx.input(|i| i.time);
        self.setup_window(frame);

        for a in self.handle_inputs(&ctx) {
            self.apply(a);
        }
        let typing = self.track_typing();
        let hidden = self.settings.hide_fullscreen && win::fullscreen_busy();
        let fg = win::foreground_window();
        let (snap, tones) = self.collect(fg);
        self.play_sounds(tones, fg, hidden);

        let waiting = snap.count(Kind::Permission) + snap.count(Kind::Waiting);
        let tooltip = match (snap.views.len(), waiting) {
            (0, _) => "ClaudeHUD: žádné relace".to_string(),
            (n, 0) => format!("ClaudeHUD: {n} {}", plural(n, "relace", "relace", "relací")),
            (n, w) => format!("ClaudeHUD: {n} {}, {w} čeká na tebe", plural(n, "relace", "relace", "relací")),
        };
        self.tray.update(summarize(&snap), tooltip);

        // What sits above the island, with a fade between changes.
        let app_card = self.app_card();
        if app_card.is_some() {
            self.last_app_card = app_card.clone();
        }
        let target = if hidden {
            Upper::None
        } else if self.panel_open {
            Upper::Panel
        } else if let Some(n) = snap.notes.first() {
            Upper::Toast(n.id)
        } else if app_card.is_some() {
            Upper::App
        } else {
            Upper::None
        };
        if let Some(n) = snap.notes.first() {
            if matches!(target, Upper::Toast(_)) {
                self.card = Some(Self::card_for_note(&snap, n));
            }
        }
        if target != self.upper {
            if self.upper == Upper::None || self.upper_t <= 0.0 {
                self.upper = target;
                self.upper_t = 0.0;
            } else {
                self.upper_t = (self.upper_t - dt * 8.0).max(0.0);
            }
        } else if self.upper != Upper::None {
            self.upper_t = (self.upper_t + dt * 6.0).min(1.0);
        }
        let animating = self.upper_t < 1.0 && self.upper != Upper::None || target != self.upper;

        // Sizes.
        let painter = ui.painter().clone();
        let pill = pill_state(&snap);
        let pill_l = pill_layout(&pill);
        let pill_w = pill_width(&painter, &pill_l);
        let older = snap.notes.len().saturating_sub(1);
        let (upper_w, upper_h) = match self.upper {
            Upper::None => (0.0, 0.0),
            Upper::Toast(_) => {
                let kind = self.card.as_ref().map_or(NoteKind::Done, |c| c.kind);
                (TOAST_W, 20.0 + card_height(kind) + if older > 0 { OLDER_H } else { 0.0 })
            }
            Upper::Panel => (PANEL_W, panel_height(panel_rows(&snap).0.len())),
            Upper::App => (TOAST_W, 130.0),
        };
        let content = vec2(pill_w.max(upper_w), PILL_H + if upper_h > 0.0 { STACK_GAP + upper_h } else { 0.0 });
        let size = if hidden { vec2(1.0, 1.0) } else { content + vec2(PAD + RIGHT_GAP, PAD + BOTTOM_GAP) };
        self.fit_window(&ctx, size);

        // Hover & click-through, from the global cursor (works while the
        // window lets clicks through and egui sees no pointer).
        let screen = ctx.content_rect();
        let anchor = pos2(screen.right() - RIGHT_GAP, screen.bottom() - BOTTOM_GAP);
        let pill_rect = Rect::from_min_max(anchor - vec2(pill_w, PILL_H), anchor);
        let upper_bottom = pill_rect.top() - STACK_GAP;
        let upper_rect = Rect::from_min_max(pos2(anchor.x - upper_w, upper_bottom - upper_h), pos2(anchor.x, upper_bottom));
        let cursor = self.cursor_local(&ctx);
        let alt = win::alt_down();
        let over_pill = pill_rect.contains(cursor);
        let over_upper = self.upper != Upper::None && upper_rect.contains(cursor);
        let pill_through = self.settings.click_through && !alt;
        let through = hidden || !(over_upper || (over_pill && !pill_through));
        if let Some(hwnd) = self.hwnd {
            win::keep_overlay(hwnd);
        }
        if self.click_through != Some(through)
            && let Some(hwnd) = self.hwnd
        {
            win::set_click_through(hwnd, through);
            self.click_through = Some(through);
        }

        // Panel closes a while after the cursor leaves it.
        if self.panel_open {
            if over_upper || over_pill {
                self.panel_touch = now;
            } else if self.panel_touch.elapsed() > PANEL_LINGER {
                self.panel_open = false;
            }
        }

        // Card timer: runs while the card is fully shown and not hovered.
        let mut drain = None;
        if let (Upper::Toast(id), Some(card)) = (self.upper, &self.card)
            && card.note == id
        {
            if self.card_for != Some(id) {
                self.card_for = Some(id);
                self.card_elapsed = 0.0;
            }
            let permission = matches!(card.kind, NoteKind::Permission(_));
            if let (false, Some(limit)) = (permission, self.settings.collapse.duration()) {
                if !over_upper && !hidden && self.upper_t >= 1.0 {
                    self.card_elapsed += dt;
                }
                let limit = limit.as_secs_f32();
                drain = Some(1.0 - self.card_elapsed / limit);
                if self.card_elapsed >= limit {
                    // Fold every card away; the island keeps showing what's unseen.
                    self.state.lock().unwrap().notes.retain(|n| matches!(n.kind, NoteKind::Permission(_)));
                }
            }
        }

        // Draw.
        let mut actions = Vec::new();
        let mut fading = false;
        if !hidden {
            let dim_target =
                if self.settings.dim_while_typing && typing && !self.panel_open && !over_upper && !over_pill { 0.3 } else { 1.0 };
            let idle_target = if pill_l.idle && !over_pill && !alt { 0.3 } else { 1.0 };
            self.dim = fade(self.dim, dim_target, dt);
            self.idle_dim = fade(self.idle_dim, idle_target, dt);
            fading = self.dim != dim_target || self.idle_dim != idle_target;
            let (dim, pill_opacity) = (self.dim, self.idle_dim);
            ui.scope(|ui| {
                ui.multiply_opacity(dim * pill_opacity);
                draw_pill(ui, pill_rect, &pill_l, &pill, time, &mut actions);
            });
            if self.upper != Upper::None && self.upper_t > 0.0 {
                let t = self.upper_t;
                let ease = 1.0 - (1.0 - t).powi(3);
                let offset = vec2(0.0, (1.0 - ease) * 14.0);
                ui.scope(|ui| {
                    ui.multiply_opacity(ease * if self.upper == Upper::Panel { 1.0 } else { dim });
                    match self.upper {
                        Upper::Panel => draw_panel(ui, upper_rect.translate(offset), &snap, &self.settings, time, &mut actions),
                        Upper::Toast(_) => {
                            if let Some(card) = &self.card {
                                let h = card_height(card.kind);
                                let bottom = upper_rect.bottom() - if older > 0 { OLDER_H } else { 0.0 };
                                let r = Rect::from_min_max(pos2(upper_rect.left(), bottom - h), pos2(upper_rect.right(), bottom));
                                draw_card(ui, r.translate(offset), card, older, drain, time, &mut actions);
                            }
                        }
                        Upper::App => {
                            if let Some(card) = &self.last_app_card {
                                draw_app_card(ui, upper_rect.translate(offset), card, time, &mut actions);
                            }
                        }
                        Upper::None => {}
                    }
                });
            }
        }
        for a in actions {
            self.apply(a);
        }

        if self.settings_open {
            self.show_settings(&ctx);
        }

        let busy = matches!(pill, Pill::Working { .. } | Pill::Mixed { .. } | Pill::Waiting { .. })
            || matches!(app_card, Some(AppCard::Update(update::Status::Installing | update::Status::Restarting)))
            || self.upper != Upper::None;
        ctx.request_repaint_after(Duration::from_millis(if hidden {
            250
        } else if animating || fading {
            8
        } else if busy {
            33
        } else {
            100
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes the tray icons as raw RGBA to $CLAUDEHUD_DUMP for eyeballing.
    #[test]
    fn dump_tray_icons() {
        let Some(dir) = std::env::var_os("CLAUDEHUD_DUMP") else { return };
        for (name, s) in [("none", None), ("idle", Some(Status::Idle)), ("working", Some(Status::Working)), ("waiting", Some(Status::Waiting)), ("done", Some(Status::Done))] {
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.rgba")), tray_rgba(s)).unwrap();
        }
    }

    #[test]
    fn initials_follow_the_design() {
        assert_eq!(ui::initials("OZBEAT"), "OZ");
        assert_eq!(ui::initials("ClaudeHUD"), "CH");
        assert_eq!(ui::initials("opensisakhk"), "OP");
        assert_eq!(ui::initials("my-app"), "MA");
    }

    #[test]
    fn durations() {
        assert_eq!(clock(Duration::from_secs(372)), "6:12");
        assert_eq!(long_duration(Duration::from_secs(372)), "6 min 12 s");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }
}
