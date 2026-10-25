//! Win32 helpers: locate the terminal window hosting a process, bring it to front.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, VK_CAPITAL, VK_CONTROL,
    VK_H, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    VK_SPACE, keybd_event,
};
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, SetWindowSubclass, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallNextHookEx, EnumWindows, KBDLLHOOKSTRUCT, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN, GW_OWNER, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindow,
    GetWindowTextW, HWND_TOPMOST, IsWindowVisible, MB_ICONASTERISK, MSG, SPI_GETWORKAREA, SW_RESTORE,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW,
    WM_HOTKEY, WM_NCCALCSIZE, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

struct Proc {
    parent: u32,
    exe: String,
}

fn process_table() -> HashMap<u32, Proc> {
    let mut map = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return map;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            map.insert(
                entry.th32ProcessID,
                Proc {
                    parent: entry.th32ParentProcessID,
                    exe: String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase(),
                },
            );
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    map
}

struct Search {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> i32 {
    unsafe {
        let search = &mut *(lparam as *mut Search);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == search.pid
            && IsWindowVisible(hwnd) != 0
            && GetWindow(hwnd, GW_OWNER).is_null()
            && GetWindowTextLengthW(hwnd) > 0
        {
            search.found = hwnd;
            return 0;
        }
        1
    }
}

fn main_window_of(pid: u32) -> HWND {
    let mut search = Search {
        pid,
        found: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(enum_cb), &mut search as *mut Search as LPARAM) };
    search.found
}

/// Walks up the parent chain of the current process and returns the first
/// ancestor that owns a visible top-level window — the terminal (Windows
/// Terminal, VS Code, conhost, ...). Falls back to the attached console window.
pub fn find_terminal_window() -> i64 {
    let procs = process_table();
    let mut pid = std::process::id();
    for _ in 0..24 {
        let Some(p) = procs.get(&pid) else { break };
        // Never "find" the desktop shell or system processes.
        if matches!(p.exe.as_str(), "explorer.exe" | "services.exe" | "wininit.exe" | "svchost.exe") {
            break;
        }
        if pid != std::process::id() {
            let hwnd = main_window_of(pid);
            if !hwnd.is_null() {
                return hwnd as i64;
            }
        }
        if p.parent == 0 || p.parent == pid {
            break;
        }
        pid = p.parent;
    }
    unsafe { GetConsoleWindow() as i64 }
}

pub fn window_exists(hwnd: i64) -> bool {
    hwnd != 0 && unsafe { IsWindow(hwnd as HWND) != 0 }
}

/// Brings a window to the foreground, restoring it if minimized.
pub fn focus_window(hwnd: i64) {
    if !window_exists(hwnd) {
        return;
    }
    let hwnd = hwnd as HWND;
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        // A synthetic Alt tap lifts the foreground lock Windows puts on
        // background processes, so SetForegroundWindow is honored.
        keybd_event(VK_MENU as u8, 0, 0, 0);
        keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
        SetForegroundWindow(hwnd);
        BringWindowToTop(hwnd);
    }
}

/// Finds a visible top-level window of this process by its exact title.
pub fn own_window(title: &str) -> Option<isize> {
    struct Find<'a> {
        pid: u32,
        title: &'a str,
        found: isize,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> i32 {
        unsafe {
            let find = &mut *(lparam as *mut Find);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == find.pid && window_title(hwnd as i64) == find.title {
                find.found = hwnd as isize;
                return 0;
            }
            1
        }
    }
    let mut find = Find { pid: std::process::id(), title, found: 0 };
    unsafe { EnumWindows(Some(cb), &mut find as *mut Find as LPARAM) };
    (find.found != 0).then_some(find.found)
}

/// No Windows 11 outline or rounded corners: the window draws its own shape
/// on a transparent canvas.
pub fn undecorate(hwnd: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        let none = DWMWA_COLOR_NONE;
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &none as *const u32 as _, 4);
        let corners = DWMWCP_DONOTROUND;
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &corners as *const i32 as _, 4);
        // Re-adding the same subclass is a no-op, so this is safe to repeat.
        SetWindowSubclass(hwnd, Some(full_client_area), 1, 0);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// The client area is the whole window. winit shifts it 1px down to make room
/// for a drop shadow on undecorated windows, which leaves a dark line along
/// the top edge of the transparent canvas.
unsafe extern "system" fn full_client_area(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCCALCSIZE && wparam != 0 {
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Makes a window a non-activating tool window: clicking it never steals focus
/// from whatever the user is typing in, and it stays out of Alt+Tab.
pub fn make_overlay(hwnd: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize) & !(WS_EX_APPWINDOW as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    }
    undecorate(hwnd as isize);
}

/// Desktop work area of the primary monitor (excludes the taskbar), in
/// physical pixels: (left, top, right, bottom).
pub fn work_area() -> (i32, i32, i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as _, 0) };
    (r.left, r.top, r.right, r.bottom)
}

/// Moves/resizes the window into the bottom-right corner of the work area
/// (physical pixels) without activating it; returns its new top-left corner.
pub fn place_bottom_right(hwnd: isize, w: i32, h: i32) -> (i32, i32) {
    let (_, _, right, bottom) = work_area();
    let (x, y) = (right - w, bottom - h);
    unsafe { SetWindowPos(hwnd as HWND, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE) };
    (x, y)
}

/// Lets mouse input fall through the window to whatever is below it. Done by
/// hand rather than via winit, which would rebuild the extended style and drop
/// the overlay bits set in [`make_overlay`].
pub fn set_click_through(hwnd: isize, on: bool) {
    let hwnd = hwnd as HWND;
    let bits = (WS_EX_TRANSPARENT | WS_EX_LAYERED) as isize;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, if on { style | bits } else { style & !bits });
    }
}

pub fn alt_down() -> bool {
    unsafe { GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000 != 0 }
}

/// Tick count of the last keyboard/mouse input anywhere in the session.
/// Counts key presses anywhere in the system, modifiers excluded (holding
/// Alt to click the island is not typing). Mouse and wheel input never count.
static KEY_PRESSES: AtomicU32 = AtomicU32::new(0);

pub fn key_presses() -> u32 {
    KEY_PRESSES.load(Ordering::Relaxed)
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let vk = unsafe { (*(lparam as *const KBDLLHOOKSTRUCT)).vkCode } as u16;
        let modifier = matches!(
            vk,
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_MENU | VK_LMENU
                | VK_RMENU | VK_LWIN | VK_RWIN | VK_CAPITAL
        );
        if !modifier {
            KEY_PRESSES.fetch_add(1, Ordering::Relaxed);
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// A full-screen game, video or presentation is in front.
pub fn fullscreen_busy() -> bool {
    let mut state = 0;
    let ok = unsafe { SHQueryUserNotificationState(&mut state) } >= 0;
    ok && matches!(state, QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
}

pub fn foreground_window() -> i64 {
    unsafe { GetForegroundWindow() as i64 }
}

#[derive(Clone, Copy, Debug)]
pub enum Hotkey {
    /// Ctrl+Alt+Space
    SwitchToDone,
    /// Ctrl+Alt+H
    TogglePanel,
}

/// Registers the global shortcuts on a thread of their own and calls
/// `on_key` for each press. Shortcuts another app already owns are skipped.
pub fn spawn_hotkeys(on_key: impl Fn(Hotkey) + Send + 'static) {
    std::thread::spawn(move || unsafe {
        let mods = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
        RegisterHotKey(std::ptr::null_mut(), 1, mods, VK_SPACE as u32);
        RegisterHotKey(std::ptr::null_mut(), 2, mods, VK_H as u32);
        // This thread pumps messages, which a low-level hook needs.
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), GetModuleHandleW(std::ptr::null()), 0);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                on_key(if msg.wParam == 1 { Hotkey::SwitchToDone } else { Hotkey::TogglePanel });
            }
        }
    });
}

pub fn window_title(hwnd: i64) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd as HWND, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Global cursor position in physical pixels.
pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

pub fn beep() {
    unsafe { MessageBeep(MB_ICONASTERISK) };
}
