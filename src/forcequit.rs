//! Force Quit (macOS-style): a small centred panel listing every running app; click a row to terminate
//! that app's processes. Opened from the Ctrl+Alt+Q hotkey. Runs on its own thread like the switcher.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use tiny_skia::{Color, FillRule, Paint, Pixmap, PixmapPaint, Transform};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::{self, Font, Surface};
use crate::shell;

const W: f32 = 360.0;
const ROW_H: f32 = 44.0;
const PAD: f32 = 12.0;
const HDR_H: f32 = 40.0;
const RADIUS: f32 = 20.0;
const MARGIN: f32 = 36.0;
const MAX_ROWS: usize = 18;
const HIDE_MS: u128 = 800;
const POLL_MS: u32 = 50;

const WM_FQ_SHOW: u32 = WM_APP + 1;
const WM_FQ_HIDE: u32 = WM_APP + 2;
const TIMER_POLL: usize = 1;

static HWND_FQ: AtomicIsize = AtomicIsize::new(0);
static SHOW: Mutex<Option<()>> = Mutex::new(None);

struct Row {
    name: String,
    pid: u32,
    icon: Option<Pixmap>,
}

struct ForceQuit {
    hwnd: HWND,
    shell: crate::shell::Shell,
    rows: Vec<Row>,
    shown: bool,
    surface: Option<Surface>,
    glass: Option<Pixmap>,
    s: f32,
    pos: (i32, i32),
    size: (u32, u32),
    hover: Option<usize>,
    idle: u128,
}

fn paint(rgb: (u8, u8, u8), a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(rgb.2, rgb.1, rgb.0, a));
    p.anti_alias = true;
    p
}

impl ForceQuit {
    fn collect(&mut self) {
        let wins = self.shell.app_windows();
        let mut seen = std::collections::HashMap::new();
        let mut rows: Vec<Row> = Vec::new();
        for w in &wins {
            let key = match (&w.aumid, &w.exe) {
                (Some(a), _) => format!("aumid:{}", a.to_lowercase()),
                (None, Some(e)) => format!("exe:{}", e.to_lowercase()),
                _ => format!("hwnd:{}", w.hwnd.0 as isize),
            };
            if seen.contains_key(&key) {
                continue;
            }
            seen.insert(key, ());
            let name = shell::friendly_name(w.exe.as_deref(), w.aumid.as_deref(), &w.title);
            let src = match (&w.aumid, &w.exe) {
                (Some(a), _) => format!("shell:AppsFolder\\{a}"),
                (None, Some(e)) => e.clone(),
                _ => String::new(),
            };
            let icon = shell::icon(&src, 48).or_else(|| w.exe.as_deref().and_then(|e| shell::icon(e, 48)));
            rows.push(Row { name, pid: w.pid, icon });
            if rows.len() >= MAX_ROWS {
                break;
            }
        }
        self.rows = rows;
    }

    fn place(&mut self) {
        let s = self.s;
        let n = self.rows.len().max(1);
        let m = MARGIN * s;
        let pw = (W * s + 2.0 * m).ceil() as u32;
        let ph = (2.0 * m + HDR_H * s + n as f32 * ROW_H * s).ceil() as u32;
        let (sx, sy) = unsafe {
            (
                (GetSystemMetrics(SM_CXSCREEN) as f32 / 2.0 - pw as f32 / 2.0).round() as i32,
                (GetSystemMetrics(SM_CYSCREEN) as f32 / 2.0 - ph as f32 / 2.0).round() as i32,
            )
        };
        self.pos = (sx, sy);
        self.size = (pw, ph);
    }

    fn build_glass(&self) -> Option<Pixmap> {
        let (w, h) = self.size;
        let mut under = gfx::capture_screen(self.pos.0, self.pos.1, w, h).or_else(|| {
            let mut p = Pixmap::new(w, h)?;
            p.fill(Color::from_rgba8(0x30, 0x30, 0x30, 0xFF));
            Some(p)
        })?;
        gfx::blur(&mut under, 14.0 * self.s);
        let mut pm = Pixmap::new(w, h)?;
        let x = MARGIN * self.s;
        let y = MARGIN * self.s;
        let pw = w as f32 - 2.0 * MARGIN * self.s;
        let ph = h as f32 - 2.0 * MARGIN * self.s;
        let r = RADIUS * self.s;
        if let Some(path) = gfx::rrect(x, y, pw, ph, r) {
            if let Some((sh, ox, oy)) = gfx::shadow(&path, 14.0 * self.s, (0, 0, 0, 0x60)) {
                pm.draw_pixmap(ox as i32, oy as i32, sh.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
        }
        if let Some((mut g, gx, gy)) = gfx::liquid_glass(&under, x, y, pw, ph, r, 10.0 * self.s, (0.0, 0.0), (0.0, 0.0)) {
            if let Some(p) = gfx::rrect(x - gx as f32, y - gy as f32, pw, ph, r) {
                let mut veil = Pixmap::new(g.width(), g.height())?;
                veil.fill_path(&p, &paint((0x14, 0x14, 0x18), 0x7A), FillRule::Winding, Transform::identity(), None);
                g.draw_pixmap(0, 0, veil.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
            pm.draw_pixmap(gx, gy, g.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        Some(pm)
    }

    fn render(&mut self) {
        if !self.shown {
            return;
        }
        if self.glass.is_none() {
            self.glass = self.build_glass();
        }
        let Some(glass) = &self.glass else { return };
        let mut pm = glass.clone();
        let s = self.s;
        let m = MARGIN * s;
        // header
        let tf = Font { face: gfx::UI, px: (15.0 * s).round() as i32, weight: 600 };
        if let Some(tt) = gfx::text_ex("Force Quit", tf, (255, 255, 255)) {
            let tw = tt.width() as f32;
            pm.draw_pixmap((m + (self.size.0 as f32 - 2.0 * m - tw) / 2.0).round() as i32, (m + (HDR_H * s - tt.height() as f32) / 2.0).round() as i32, tt.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        for (i, r) in self.rows.iter().enumerate() {
            let y = m + HDR_H * s + i as f32 * ROW_H * s;
            let on = self.hover == Some(i);
            if let Some(p) = gfx::rrect(m + 2.0 * s, y, self.size.0 as f32 - 2.0 * (m + 2.0 * s), ROW_H * s - 4.0 * s, 9.0 * s) {
                pm.fill_path(&p, &paint((255, 255, 255), if on { 0x22 } else { 0x0C }), FillRule::Winding, Transform::identity(), None);
            }
            if let Some(ic) = &r.icon {
                let d = 28.0 * s;
                let k = d / ic.width().max(1) as f32;
                let tf2 = Transform::from_row(k, 0.0, 0.0, k, m + 8.0 * s, y + (ROW_H * s - d) / 2.0);
                pm.draw_pixmap(0, 0, ic.as_ref(), &PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() }, tf2, None);
            }
            let f = Font { face: gfx::UI, px: (14.0 * s).round() as i32, weight: 400 };
            if let Some(tt) = gfx::text_ex(&r.name, f, (240, 240, 245)) {
                pm.draw_pixmap((m + 48.0 * s).round() as i32, (y + (ROW_H * s - tt.height() as f32) / 2.0).round() as i32, tt.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
        }
        if self.surface.as_ref().is_none_or(|sf| (sf.w, sf.h) != self.size) {
            self.surface = Surface::new(self.size.0, self.size.1);
        }
        if let Some(sf) = self.surface.as_mut() {
            sf.bytes().copy_from_slice(pm.data());
            sf.present(self.hwnd, self.pos.0, self.pos.1);
        }
    }

    fn kill(&self, i: usize) {
        if let Some(r) = self.rows.get(i) {
            unsafe {
                if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, r.pid) {
                    let _ = TerminateProcess(h, 1);
                    let _ = CloseHandle(h);
                }
            }
        }
    }

    fn show(&mut self) {
        let _guard = SHOW.lock();
        self.collect();
        if self.rows.is_empty() {
            return;
        }
        self.place();
        self.shown = true;
        self.glass = None;
        self.hover = None;
        self.idle = 0;
        self.render();
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, self.pos.0, self.pos.1, self.size.0 as i32, self.size.1 as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    fn hide(&mut self) {
        self.shown = false;
        self.rows.clear();
        self.glass = None;
        self.surface = None;
        self.hover = None;
        self.idle = 0;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn poll(&mut self) {
        if !self.shown {
            return;
        }
        let mut p = windows::Win32::Foundation::POINT::default();
        unsafe {
            if GetCursorPos(&mut p).is_err() {
                return;
            }
        }
        let inside = p.x >= self.pos.0 && p.x < self.pos.0 + self.size.0 as i32 && p.y >= self.pos.1 && p.y < self.pos.1 + self.size.1 as i32;
        if inside {
            self.idle = 0;
        } else {
            self.idle += POLL_MS as u128;
            if self.idle > HIDE_MS {
                self.hide();
            }
        }
    }
}

thread_local! {
    static STATE: std::cell::RefCell<Option<ForceQuit>> = const { std::cell::RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut ForceQuit) -> R) -> Option<R> {
    STATE.with(|st| st.borrow_mut().as_mut().map(f))
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_FQ_SHOW => { with(|f| f.show()); return LRESULT(0); }
        WM_FQ_HIDE => { with(|f| f.hide()); return LRESULT(0); }
        WM_TIMER if wp.0 == TIMER_POLL => { with(|f| f.poll()); return LRESULT(0); }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
            with(|f| {
                let s = f.s;
                let m = MARGIN * s;
                let mut hover = None;
                for i in 0..f.rows.len() {
                    let top = m + HDR_H * s + i as f32 * ROW_H * s;
                    if x >= m && x < f.size.0 as f32 - m && y >= top && y < top + ROW_H * s {
                        hover = Some(i);
                    }
                }
                if hover != f.hover {
                    f.hover = hover;
                    f.render();
                } else {
                    f.hover = hover;
                }
            });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
            with(|f| {
                let s = f.s;
                let m = MARGIN * s;
                let mut killed = None;
                for i in 0..f.rows.len() {
                    let top = m + HDR_H * s + i as f32 * ROW_H * s;
                    if x >= m && x < f.size.0 as f32 - m && y >= top && y < top + ROW_H * s {
                        killed = Some(i);
                    }
                }
                if let Some(i) = killed {
                    f.kill(i);
                    f.hide();
                } else {
                    f.hide(); // click outside the list closes
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(h, msg, wp, lp)
}

pub fn start() {
    std::thread::spawn(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("WinMacForceQuit"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            w!("WinMacForceQuit"), w!("WIN MAC Force Quit"), WS_POPUP, 0, 0, 1, 1, HWND::default(), HMENU::default(), hinst, None,
        ) else { return };
        HWND_FQ.store(hwnd.0 as isize, Ordering::Relaxed);
        let dpi = GetDpiForWindow(hwnd);
        let fq = ForceQuit {
            hwnd, shell: crate::shell::Shell::default(), rows: Vec::new(), shown: false, surface: None, glass: None,
            s: dpi as f32 / 96.0, pos: (0, 0), size: (1, 1), hover: None, idle: 0,
        };
        STATE.with(|st| *st.borrow_mut() = Some(fq));
        let _ = SetTimer(hwnd, TIMER_POLL, POLL_MS, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

pub fn show() {
    let h = HWND_FQ.load(Ordering::Relaxed);
    if h != 0 {
        unsafe { let _ = PostMessageW(HWND(h as *mut _), WM_FQ_SHOW, WPARAM(0), LPARAM(0)); }
    }
}
