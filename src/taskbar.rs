//! Windows-taskbar handling.
//!
//! HideTaskbar=true: we reserve our strip as an AppBar and ask Explorer to autohide its own taskbar
//! via ABM_SETSTATE. Explorer does the actual hiding and keeps the work area correct, so maximized
//! windows stop above the dock instead of covering it.
//!
//! We deliberately never call ShowWindow(SW_HIDE) on Shell_TrayWnd: that collapses Explorer's work
//! area to 0x0,0x0 and makes every maximized window cover the dock.

use std::cell::Cell;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETSTATE, ABM_SETSTATE, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, ShowWindow, SW_HIDE,
};

const ABS_AUTOHIDE: isize = 1;

thread_local! {
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
    static ORIGINAL: Cell<isize> = const { Cell::new(0) };
    static WATCH: Cell<(isize, u32)> = const { Cell::new((0, 0)) };
}

const EVENT_OBJECT_SHOW: u32 = 0x8002;
const WINEVENT_OUTOFCONTEXT: u32 = 0;

/// Explorer's own taskbar, skipping any Shell_TrayWnd this process registers (a future tray host).
fn main_bar() -> Option<HWND> {
    let me = std::process::id();
    let mut h = HWND::default();
    loop {
        h = unsafe { FindWindowExW(HWND::default(), h, w!("Shell_TrayWnd"), None) }.ok()?;
        if h.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        if pid != me {
            return Some(h);
        }
    }
}

/// The pid of the Explorer that owns the main taskbar (changes when Explorer restarts).
pub fn explorer_pid() -> Option<u32> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(main_bar()?, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// Hide the Windows taskbar: set ABM_SETSTATE autohide (so Explorer keeps a sensible work area) and
/// also hide the bar window itself, so it does not peek up out of the dock's strip.
pub fn hide() {
    let Some(main) = main_bar() else { return };
    unsafe {
        let mut d = data(main, 0);
        let state = SHAppBarMessage(ABM_GETSTATE, &mut d) as isize;
        if !HIDDEN.get() {
            ORIGINAL.set(state);
        }
        if state & ABS_AUTOHIDE == 0 {
            let mut d = data(main, state | ABS_AUTOHIDE);
            SHAppBarMessage(ABM_SETSTATE, &mut d);
        }
        if IsWindowVisible(main).as_bool() {
            let _ = ShowWindow(main, SW_HIDE);
        }
    }
    HIDDEN.set(true);
    watch(main);
}

/// Explorer re-shows the taskbar after restarts or display changes.
pub fn keep_hidden() {
    if HIDDEN.get() && bars().into_iter().any(|b| unsafe { IsWindowVisible(b) }.as_bool()) {
        hide();
    }
}

/// `force` puts the taskbar back even if this process never hid it.
///
/// Note: in a fresh process (e.g. `winmac.exe --restore` after a crash, or the panic hook)
/// HIDDEN is false and ORIGINAL is 0, so replaying ORIGINAL would do nothing. When `force` is
/// set we therefore read the live state and clear only the autohide bit, leaving the user's
/// other AppBar bits untouched.
pub fn restore(force: bool) {
    let hidden = HIDDEN.get();
    if !hidden && !force {
        return;
    }
    unwatch();
    let Some(main) = main_bar() else { return };
    unsafe {
        let mut d = data(main, 0);
        let live = SHAppBarMessage(ABM_GETSTATE, &mut d) as isize;
        let target = if hidden { ORIGINAL.get() } else { live & !ABS_AUTOHIDE };
        let mut d = data(main, target);
        SHAppBarMessage(ABM_SETSTATE, &mut d);
    }
    HIDDEN.set(false);
}

/// Explorer shows the taskbar again on its own (leaving a fullscreen app, display changes): re-apply
/// the autohide state instead of forcing ShowWindow(SW_HIDE) — the latter corrupts the work area to
/// 0x0,0x0 and makes maximized windows cover the dock.
unsafe extern "system" fn on_show(_: HWINEVENTHOOK, _: u32, h: HWND, obj: i32, _: i32, _: u32, _: u32) {
    if obj == 0 && !h.0.is_null() && HIDDEN.get() && is_bar(h) {
        let Some(main) = main_bar() else { return };
        let mut d = data(main, 0);
        let state = SHAppBarMessage(ABM_GETSTATE, &mut d) as isize;
        if state & ABS_AUTOHIDE == 0 {
            let mut d = data(main, state | ABS_AUTOHIDE);
            SHAppBarMessage(ABM_SETSTATE, &mut d);
        }
        if IsWindowVisible(main).as_bool() {
            let _ = ShowWindow(main, SW_HIDE);
        }
    }
}

fn is_bar(h: HWND) -> bool {
    let mut buf = [0u16; 32];
    let n = unsafe { GetClassNameW(h, &mut buf) } as usize;
    let c = String::from_utf16_lossy(&buf[..n]);
    c == "Shell_TrayWnd" || c == "Shell_SecondaryTrayWnd"
}

fn watch(main: HWND) {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(main, Some(&mut pid)) };
    let (hook, cur) = WATCH.get();
    if pid == 0 || (pid == cur && hook != 0) {
        return;
    }
    unwatch();
    let h = unsafe { SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(on_show), pid, 0, WINEVENT_OUTOFCONTEXT) };
    WATCH.set((h.0 as isize, pid));
}

fn unwatch() {
    let (hook, _) = WATCH.get();
    if hook != 0 {
        unsafe {
            let _ = UnhookWinEvent(HWINEVENTHOOK(hook as *mut _));
        }
    }
    WATCH.set((0, 0));
}

fn bars() -> Vec<HWND> {
    let mut v: Vec<HWND> = main_bar().into_iter().collect();
    let mut h = HWND::default();
    while let Ok(n) = unsafe { FindWindowExW(HWND::default(), h, w!("Shell_SecondaryTrayWnd"), None) } {
        if n.0.is_null() {
            break;
        }
        v.push(n);
        h = n;
    }
    v
}

fn data(h: HWND, state: isize) -> APPBARDATA {
    APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: h, uCallbackMessage: 0, uEdge: 0, lParam: LPARAM(state), ..Default::default() }
}
