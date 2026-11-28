//! Dock hover window previews (DockDoor-style). Hover a running app icon in the dock and a frosted panel
//! opens above the dock with a live (PrintWindow) thumbnail of each open window of that app, an app header
//! (icon + name), a traffic-light close orb on each thumbnail; click a thumbnail to switch to it, and
//! middle-click — or the close orb — closes a window without switching.
//!
//! Runs on its own thread like the switcher, so the dock's render loop never blocks. The panel appears
//! immediately with placeholder tiles and fades in; the thumbnails and the glass backdrop are built together
//! on a worker thread so the first real paint is ready as soon as the grabs return. A generation counter
//! discards stale grabs after the target changed or the panel closed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, SpreadMode, Stroke, Transform};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::{self, Font, Surface};
use crate::shell;

// DIPs
const THUMB_W: f32 = 250.0; // widest a thumbnail gets
const THUMB_MIN: f32 = 120.0;
const THUMB_ASPECT: f32 = 1.6; // fixed tile aspect keeps the panel geometry stable (no jump/rebuild)
const MAX_WINDOWS: usize = 8;
const PAD: f32 = 16.0;
const GAP: f32 = 20.0;
const HDR_H: f32 = 46.0;
const TITLE_H: f32 = 22.0;
const RADIUS: f32 = 18.0;
const MARGIN: f32 = 36.0;
const ORB: f32 = 16.0;
const GAP_BELOW: f32 = 10.0;
const HIDE_MS: u128 = 450;
const POLL_MS: u32 = 80;
const FADE_MS: f32 = 160.0; // entrance fade
/// hold the pointer on an icon this long before a grab starts; rapid travel across icons never grabs
const DEBOUNCE_MS: u64 = 170;

const WM_PV_SHOW: u32 = WM_APP + 1;
const WM_PV_HIDE: u32 = WM_APP + 2;
const WM_PV_READY: u32 = WM_APP + 3;
const TIMER_POLL: usize = 1;

static HWND_PV: AtomicIsize = AtomicIsize::new(0);
static GEN: AtomicU64 = AtomicU64::new(0);

/// What the dock wants a preview to show.
#[derive(Clone)]
pub struct ShowReq {
    pub wins: Vec<isize>,
    pub name: String,
    pub icon_spec: Option<String>,
    pub cx: i32, // screen px centre of the hovered dock icon
    pub bottom: i32, // screen px y of the dock plate top
    pub dock: (i32, i32, i32, i32), // screen px rect of the dock strip (keep-alive)
}

static SHOW_REQ: Mutex<Option<ShowReq>> = Mutex::new(None);

/// The single atomic (per-generation) hand-off from a grab worker back to the preview thread: thumbs, header
/// icon and glass are written together under one lock so a slow worker can never tear the state of a newer one.
struct ReadyPayload {
    gen: u64,
    thumbs: Option<Vec<(isize, String, Pixmap)>>,
    icon: Option<Pixmap>,
    glass: Option<Pixmap>,
}
static READY: Mutex<Option<ReadyPayload>> = Mutex::new(None);

/// Per-window thumbnail cache: re-hovering an app within the TTL reuses its grabs (no PrintWindow), which is
/// by far the most expensive part of a preview.
struct ThumbCache {
    img: Pixmap,
    at: Instant,
}
const THUMB_TTL_MS: u128 = 3000;
const THUMB_CACHE_MAX: usize = 48;
static THUMB_CACHE: std::sync::OnceLock<Mutex<HashMap<isize, ThumbCache>>> = std::sync::OnceLock::new();
fn thumb_cache() -> &'static Mutex<HashMap<isize, ThumbCache>> {
    THUMB_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One pending grab job; a newer show overwrites this, so rapid hover changes coalesce to the latest target.
struct PendingJob {
    gen: u64,
    wins: Vec<isize>,
    max_w: i32,
    icon_spec: Option<String>,
    pos: (i32, i32),
    size: (u32, u32),
    s: f32,
}
static JOB: Mutex<Option<PendingJob>> = Mutex::new(None);
static JOB_CV: Condvar = Condvar::new();

struct Thumb {
    hwnd: isize,
    title: String,
    img: Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    placeholder: bool,
}

struct Preview {
    hwnd: HWND,
    req: Option<ShowReq>,
    thumbs: Vec<Thumb>,
    icon: Option<Pixmap>,
    shown: bool,
    surface: Option<Surface>,
    glass: Option<Pixmap>,
    s: f32,
    pos: (i32, i32),
    size: (u32, u32),
    hover: Option<usize>,
    close_hov: Option<usize>,
    idle: u128,
    pending_gen: u64,
    alpha: f32,
    /// signature of the target currently shown; identical repeat shows are skipped
    sig: u64,
    /// the target we would like to grab (after a debounce) vs the one already enqueued
    want_sig: u64,
    job_sig: u64,
    deadline: Option<Instant>,
    /// last built glass, keyed by a ~64px position bucket + size: nearby icons share it
    cached: Option<((i64, i64, i64), Pixmap)>,
}

fn paint(rgb: (u8, u8, u8), a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(rgb.2, rgb.1, rgb.0, a)); // BGRA surfaces
    p.anti_alias = true;
    p
}

fn mix(c: (u8, u8, u8), target: u8, k: f32) -> (u8, u8, u8) {
    let k = k.clamp(0.0, 1.0);
    (
        (c.0 as f32 + (target as f32 - c.0 as f32) * k).round() as u8,
        (c.1 as f32 + (target as f32 - c.1 as f32) * k).round() as u8,
        (c.2 as f32 + (target as f32 - c.2 as f32) * k).round() as u8,
    )
}

fn stroke_line(pm: &mut Pixmap, x0: f32, y0: f32, x1: f32, y1: f32, c: Paint<'static>, w: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);
    pb.line_to(x1, y1);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &c, &Stroke { width: w, ..Default::default() }, Transform::identity(), None);
    }
}

fn fit_title(s: &str, max_w: f32, f: Font) -> String {
    if gfx::measure(s, f).0 as f32 <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        let mut cand = out.clone();
        cand.push(ch);
        if gfx::measure(&cand, f).0 as f32 > max_w {
            break;
        }
        out = cand;
    }
    if out.is_empty() { s.chars().next().map(|c| c.to_string()).unwrap_or_default() } else { out }
}

fn window_title(h: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(HWND(h as *mut _), &mut buf) };
    if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { String::new() }
}

fn placeholder_img() -> Option<Pixmap> {
    let mut p = Pixmap::new(16, 10)?;
    p.fill(Color::from_rgba8(0x2A, 0x2A, 0x30, 0xFF));
    Some(p)
}

fn glass_key(pos: (i32, i32), size: (u32, u32)) -> (i64, i64, i64) {
    ((pos.0 >> 6) as i64, (pos.1 >> 6) as i64, (size.0 as u64 * 64 + size.1 as u64) as i64)
}

fn monitor_rect(cx: i32, cy: i32) -> (i32, i32, i32, i32) {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: cx, y: cy }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            (mi.rcMonitor.left, mi.rcMonitor.top, mi.rcMonitor.right, mi.rcMonitor.bottom)
        } else {
            (cx - 2000, cy - 2000, cx + 2000, cy + 2000)
        }
    }
}

/// A macOS-style traffic-light orb: coloured glass sphere with a specular highlight and a glyph.
fn draw_orb(pm: &mut Pixmap, cx: f32, cy: f32, d: f32, base: (u8, u8, u8), glyph: (u8, u8, u8), over: bool) {
    if d <= 0.0 {
        return;
    }
    let r = d / 2.0;
    if let Some(p) = gfx::rrect(cx - r, cy - r, d, d, r) {
        let top = if over { mix(base, 255, 0.42) } else { mix(base, 255, 0.25) };
        let bot = mix(base, 0, 0.30);
        let mut body = Paint::default();
        if let Some(grad) = LinearGradient::new(
            tiny_skia::Point::from_xy(cx, cy - r),
            tiny_skia::Point::from_xy(cx, cy + r),
            vec![GradientStop::new(0.0, Color::from_rgba8(top.2, top.1, top.0, 255)), GradientStop::new(1.0, Color::from_rgba8(bot.2, bot.1, bot.0, 255))],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            body.shader = grad;
        }
        pm.fill_path(&p, &body, FillRule::Winding, Transform::identity(), None);
        if let Some(mut mask) = tiny_skia::Mask::new(pm.width(), pm.height()) {
            mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
            let mut hl = Paint::default();
            if let Some(grad) = LinearGradient::new(
                tiny_skia::Point::from_xy(cx, cy - r),
                tiny_skia::Point::from_xy(cx, cy - r + r * 1.2),
                vec![
                    GradientStop::new(0.0, Color::from_rgba8(255, 255, 255, if over { 90 } else { 55 })),
                    GradientStop::new(1.0, Color::from_rgba8(255, 255, 255, 0)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                hl.shader = grad;
            }
            pm.fill_rect(Rect::from_xywh(cx - r, cy - r, d, d).expect("orb rect"), &hl, Transform::identity(), Some(&mask));
        }
        pm.stroke_path(&p, &paint(mix(base, 0, 0.55), 90), &Stroke { width: 0.5f32.max(d / 28.0), ..Default::default() }, Transform::identity(), None);
    }
    let k = d * 0.28;
    let lw = d * 0.13;
    let g = paint(glyph, 255);
    stroke_line(pm, cx - k, cy - k, cx + k, cy + k, g.clone(), lw);
    stroke_line(pm, cx - k, cy + k, cx + k, cy - k, g, lw);
}

/// Shadow + frosted dark glass under a panel at `pos`/`size` (screen px), scaled by `s`.
fn build_glass(pos: (i32, i32), size: (u32, u32), s: f32) -> Option<Pixmap> {
    let (w, h) = size;
    let mut under = gfx::capture_screen(pos.0, pos.1, w, h).or_else(|| {
        let mut p = Pixmap::new(w, h)?;
        p.fill(Color::from_rgba8(0x30, 0x30, 0x30, 0xFF));
        Some(p)
    })?;
    gfx::blur(&mut under, 12.0 * s);
    let mut pm = Pixmap::new(w, h)?;
    let x = MARGIN * s;
    let y = MARGIN * s;
    let pw = w as f32 - 2.0 * MARGIN * s;
    let ph = h as f32 - 2.0 * MARGIN * s;
    let r = RADIUS * s;
    if let Some(path) = gfx::rrect(x, y + 6.0 * s, pw, ph, r) {
        if let Some((sh, ox, oy)) = gfx::shadow(&path, 12.0 * s, (0, 0, 0, 0x60)) {
            pm.draw_pixmap(ox as i32, oy as i32, sh.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }
    if let Some((mut g, gx, gy)) = gfx::liquid_glass(&under, x, y, pw, ph, r, 10.0 * s, (0.0, 0.0), (0.0, 0.0)) {
        if let Some(p) = gfx::rrect(x - gx as f32, y - gy as f32, pw, ph, r) {
            let mut veil = Pixmap::new(g.width(), g.height())?;
            veil.fill_path(&p, &paint((0x16, 0x16, 0x18), 0x78), FillRule::Winding, Transform::identity(), None);
            g.draw_pixmap(0, 0, veil.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        pm.draw_pixmap(gx, gy, g.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    Some(pm)
}

/// A cheap static glass (no screen capture): used the instant a preview appears, so the first paint never
/// blocks on a capture+blur. The worker replaces it with the real frosted glass a few ms later.
fn placeholder_glass(pos: (i32, i32), size: (u32, u32), s: f32) -> Option<Pixmap> {
    let (w, h) = size;
    let mut pm = Pixmap::new(w, h)?;
    let x = MARGIN * s;
    let y = MARGIN * s;
    let pw = w as f32 - 2.0 * MARGIN * s;
    let ph = h as f32 - 2.0 * MARGIN * s;
    let r = RADIUS * s;
    if let Some(path) = gfx::rrect(x, y + 6.0 * s, pw, ph, r) {
        if let Some((sh, ox, oy)) = gfx::shadow(&path, 16.0 * s, (0, 0, 0, 0x60)) {
            pm.draw_pixmap(ox as i32, oy as i32, sh.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }
    let mut body = Paint::default();
    if let Some(grad) = LinearGradient::new(
        tiny_skia::Point::from_xy(x, y),
        tiny_skia::Point::from_xy(x, y + ph),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(0x24, 0x24, 0x2A, 0xF0)),
            GradientStop::new(1.0, Color::from_rgba8(0x11, 0x11, 0x15, 0xE8)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        body.shader = grad;
    }
    if let Some(p) = gfx::rrect(x, y, pw, ph, r) {
        pm.fill_path(&p, &body, FillRule::Winding, Transform::identity(), None);
    }
    let _ = pos;
    Some(pm)
}

/// The single grab worker: waits for a coalesced job, serves thumbnails from the per-window cache (only
/// PrintWindow-grabbing the misses), builds the glass, and hands the result over as one atomic `ReadyPayload`.
fn worker_loop() {
    loop {
        let job = {
            let mut j = JOB.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(job) = j.take() {
                    break job;
                }
                j = JOB_CV.wait(j).unwrap_or_else(|e| e.into_inner());
            }
        };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let now = Instant::now();
        let mut misses: Vec<isize> = Vec::new();
        {
            let c = thumb_cache().lock().unwrap_or_else(|e| e.into_inner());
            for &w in &job.wins {
                let fresh = c.get(&w).is_some_and(|t| now.duration_since(t.at).as_millis() < THUMB_TTL_MS);
                if !fresh {
                    misses.push(w);
                }
            }
        }
        let got = crate::genie::preview_thumbs(&misses, job.max_w);
        {
            let mut c = thumb_cache().lock().unwrap_or_else(|e| e.into_inner());
            for (k, img) in &got {
                c.insert(*k, ThumbCache { img: img.clone(), at: now });
            }
            while c.len() > THUMB_CACHE_MAX {
                let oldest = c.iter().min_by_key(|(_, v)| v.at).map(|(k, _)| *k);
                if let Some(k) = oldest {
                    c.remove(&k);
                } else {
                    break;
                }
            }
        }
        let mut out: Vec<(isize, String, Pixmap)> = Vec::new();
        {
            let c = thumb_cache().lock().unwrap_or_else(|e| e.into_inner());
            for &w in &job.wins {
                if let Some(t) = c.get(&w) {
                    out.push((w, window_title(w), t.img.clone()));
                }
            }
        }
        let icon = job.icon_spec.as_deref().and_then(|spec| shell::icon(spec, 48));
        let glass = build_glass(job.pos, job.size, job.s);
        if let Ok(mut slot) = READY.lock() {
            *slot = Some(ReadyPayload {
                gen: job.gen,
                thumbs: if out.is_empty() { None } else { Some(out) },
                icon,
                glass,
            });
        }
        unsafe {
            CoUninitialize();
            let h = HWND_PV.load(Ordering::Relaxed);
            if h != 0 {
                let _ = PostMessageW(HWND(h as *mut _), WM_PV_READY, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Preview {
    fn place(&mut self) {
        let s = self.s;
        let n = self.thumbs.len().max(1) as f32;
        let Some(req) = &self.req else { return };
        let avail = ((req.dock.2 - req.dock.0) as f32 / s - 2.0 * PAD).max(1.0);
        let twin = ((avail - (n - 1.0) * GAP) / n).clamp(THUMB_MIN, THUMB_W);
        let th = twin / THUMB_ASPECT;
        let row_h = th + TITLE_H;
        let panel_w = n * twin + (n - 1.0) * GAP;
        let m = MARGIN * s;
        let pw = (panel_w + 2.0 * m).ceil() as u32;
        let ph = (2.0 * m + HDR_H * s + row_h * s).ceil() as u32;
        let mut x = m;
        let ty = m + HDR_H * s;
        for t in &mut self.thumbs {
            t.x = x;
            t.y = ty;
            t.w = twin * s;
            t.h = row_h * s;
            x += t.w + GAP * s;
        }
        let mr = monitor_rect(req.cx, req.bottom);
        let left = ((req.cx as f32 - pw as f32 / 2.0).round() as i32).clamp(mr.0, (mr.2 - pw as i32).max(mr.0));
        let top = ((req.bottom as f32 - ph as f32 - GAP_BELOW * s).round() as i32).clamp(mr.1, (mr.3 - ph as i32).max(mr.1));
        self.pos = (left, top);
        self.size = (pw, ph);
    }

    fn draw_header(&self, pm: &mut Pixmap) {
        let s = self.s;
        let m = MARGIN * s;
        let x0 = m + PAD * s;
        let y0 = m + (HDR_H * s - 24.0 * s) / 2.0;
        if let Some(icon) = &self.icon {
            let d = 24.0 * s;
            let k = d / icon.width().max(1) as f32;
            let tf = Transform::from_row(k, 0.0, 0.0, k, x0, y0);
            pm.draw_pixmap(0, 0, icon.as_ref(), &PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() }, tf, None);
        }
        if let Some(name) = self.req.as_ref().map(|r| r.name.as_str()) {
            let tf = Font { face: gfx::UI, px: (14.0 * s).round() as i32, weight: 500 };
            let name = fit_title(name, self.size.0 as f32 - 2.0 * m - 40.0 * s, tf);
            if let Some(tt) = gfx::text_ex(&name, tf, (255, 255, 255)) {
                let tx = x0 + 24.0 * s + 10.0 * s;
                let ty = y0 + (24.0 * s - tt.height() as f32) / 2.0;
                pm.draw_pixmap(tx.round() as i32, ty.round() as i32, tt.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
        }
        let y = m + HDR_H * s - 2.0 * s;
        stroke_line(pm, m, y, self.size.0 as f32 - m, y, paint((255, 255, 255), 26), 1.0 * s);
    }

    fn draw_thumb(&self, pm: &mut Pixmap, t: &Thumb, hilite: bool) {
        let s = self.s;
        let r = 12.0 * s;
        if let Some(p) = gfx::rrect(t.x, t.y, t.w, t.h, r) {
            pm.fill_path(&p, &paint((0x1A, 0x1A, 0x1E), 0xE0), FillRule::Winding, Transform::identity(), None);
        }
        let img_y = t.y;
        let img_h = t.h - TITLE_H * s;
        let inset = 4.0 * s;
        let ix = t.x + inset;
        let iw = t.w - 2.0 * inset;
        let ih = img_h - 2.0 * inset;
        if iw > 1.0 && ih > 1.0 {
            if t.placeholder {
                // a soft skeleton tile while the real grab is on its way
                let mut ph = Paint::default();
                if let Some(grad) = LinearGradient::new(
                    tiny_skia::Point::from_xy(0.0, img_y + inset),
                    tiny_skia::Point::from_xy(0.0, img_y + ih),
                    vec![
                        GradientStop::new(0.0, Color::from_rgba8(0x34, 0x34, 0x3C, 0xD0)),
                        GradientStop::new(1.0, Color::from_rgba8(0x20, 0x20, 0x26, 0xD0)),
                    ],
                    SpreadMode::Pad,
                    Transform::identity(),
                ) {
                    ph.shader = grad;
                }
                if let Some(p) = gfx::rrect(ix, img_y + inset, iw, ih, r) {
                    pm.fill_path(&p, &ph, FillRule::Winding, Transform::identity(), None);
                }
            } else {
                // letterbox: fit the whole window inside the box without stretching
                if let Some(mut mask) = tiny_skia::Mask::new(pm.width(), pm.height()) {
                    if let Some(p) = gfx::rrect(ix, img_y + inset, iw, ih, r) {
                        mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
                    }
                    let k = (iw / t.img.width() as f32).min(ih / t.img.height() as f32);
                    let dw = t.img.width() as f32 * k;
                    let dh = t.img.height() as f32 * k;
                    let dx = ix + (iw - dw) / 2.0;
                    let dy = img_y + inset + (ih - dh) / 2.0;
                    let tf = Transform::from_row(k, 0.0, 0.0, k, dx, dy);
                    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() };
                    pm.draw_pixmap(0, 0, t.img.as_ref(), &paint, tf, Some(&mask));
                }
            }
        }
        // a soft scrim at the bottom of the image so the title reads over bright windows
        let scr_top = img_y + ih - 30.0 * s;
        if scr_top < img_y + ih {
            let mut g = Paint::default();
            if let Some(grad) = LinearGradient::new(
                tiny_skia::Point::from_xy(0.0, scr_top),
                tiny_skia::Point::from_xy(0.0, img_y + ih),
                vec![
                    GradientStop::new(0.0, Color::from_rgba8(0, 0, 0, 0)),
                    GradientStop::new(1.0, Color::from_rgba8(0, 0, 0, 0xA0)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                g.shader = grad;
            }
            if let Some(rr) = Rect::from_xywh(ix, scr_top, iw, img_y + ih - scr_top) {
                pm.fill_rect(rr, &g, Transform::identity(), None);
            }
        }
        if hilite {
            if let Some(p) = gfx::rrect(t.x + 1.0 * s, t.y + 1.0 * s, t.w - 2.0 * s, t.h - 2.0 * s, r) {
                pm.fill_path(&p, &paint((255, 255, 255), 0x14), FillRule::Winding, Transform::identity(), None);
            }
        }
        if !t.placeholder {
            let tf = Font { face: gfx::UI, px: (12.0 * s).round() as i32, weight: 500 };
            let title = fit_title(&t.title, (t.w - 16.0 * s).max(1.0), tf);
            if let Some(tt) = gfx::text_ex(&title, tf, (240, 240, 245)) {
                let tx = t.x + 8.0 * s;
                let ty = t.y + t.h - TITLE_H * s + (TITLE_H * s - tt.height() as f32) / 2.0;
                pm.draw_pixmap(tx.round() as i32, ty.round() as i32, tt.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
        }
        let orb = ORB * s;
        let ox = t.x + t.w - orb - 7.0 * s;
        let oy = t.y + 7.0 * s;
        let over = self.close_hov == self.thumbs.iter().position(|q| std::ptr::eq(q, t));
        draw_orb(pm, ox + orb / 2.0, oy + orb / 2.0, orb, (0xA2, 0x1B, 0x1B), (0x45, 0x00, 0x00), over);
    }

    fn render(&mut self) {
        if !self.shown {
            return;
        }
        if self.glass.is_none() {
            self.glass = placeholder_glass(self.pos, self.size, self.s); // never block on a live capture here
        }
        let Some(glass) = &self.glass else { return };
        let mut pm = glass.clone();
        self.draw_header(&mut pm);
        for (i, t) in self.thumbs.iter().enumerate() {
            self.draw_thumb(&mut pm, t, self.hover == Some(i));
        }
        if self.surface.as_ref().is_none_or(|sf| (sf.w, sf.h) != self.size) {
            self.surface = Surface::new(self.size.0, self.size.1);
        }
        if let Some(sf) = self.surface.as_mut() {
            sf.bytes().copy_from_slice(pm.data());
            if self.alpha < 255.0 {
                sf.present_alpha(self.hwnd, self.pos.0, self.pos.1, self.alpha as u8);
            } else {
                sf.present(self.hwnd, self.pos.0, self.pos.1);
            }
        }
    }

    fn show(&mut self) {
        let req = SHOW_REQ.lock().map(|g| g.clone()).ok().flatten();
        let Some(req) = req else { return };
        if req.wins.is_empty() {
            self.end();
            return;
        }
        // a signature of this target; skip identical repeat shows (the dock now offers every frame)
        let mut sig: u64 = 0xcbf29ce484222325;
        for &w in &req.wins {
            sig ^= w as u64;
            sig = sig.wrapping_mul(0x100000001b3);
        }
        for &b in req.name.as_bytes() {
            sig ^= b as u64;
            sig = sig.wrapping_mul(0x100000001b3);
        }
        // NB: the signature deliberately excludes geometry (cx/bottom) — it shifts every frame with the
        // magnification easing, which would defeat the dedup and spawn a worker each frame.
        if self.shown && self.sig == sig {
            return;
        }
        self.sig = sig;
        self.req = Some(req.clone());
        // show placeholder tiles immediately for instant feedback and a stable geometry
        let count = req.wins.len().min(MAX_WINDOWS);
        self.thumbs = (0..count)
            .map(|_| Thumb {
                hwnd: 0,
                title: String::new(),
                img: placeholder_img().unwrap_or_else(|| Pixmap::new(16, 10).unwrap()),
                x: 0.0, y: 0.0, w: 0.0, h: 0.0,
                placeholder: true,
            })
            .collect();
        self.place();
        self.shown = true;
        let key = glass_key(self.pos, self.size);
        self.glass = match &self.cached {
            Some((k, pm)) if *k == key => Some(pm.clone()),
            _ => placeholder_glass(self.pos, self.size, self.s),
        };
        self.hover = None;
        self.close_hov = None;
        self.idle = 0;
        self.alpha = 0.0;
        self.render();
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, self.pos.0, self.pos.1, self.size.0 as i32, self.size.1 as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }

        self.want_sig = sig;
        self.deadline = Some(Instant::now() + Duration::from_millis(DEBOUNCE_MS));
    }

    /// Enqueue the debounced grab for the currently shown target (called from the poll timer once the
    /// pointer has held still on this target beyond the debounce, or the target changed and the old grab
    /// is already known).
    fn enqueue_job(&mut self) {
        let Some(req) = &self.req else { return };
        let g = GEN.fetch_add(1, Ordering::Relaxed) + 1;
        self.pending_gen = g;
        let job = PendingJob {
            gen: g,
            wins: req.wins.iter().take(MAX_WINDOWS).copied().collect(),
            max_w: (THUMB_W * self.s) as i32,
            icon_spec: req.icon_spec.clone(),
            pos: self.pos,
            size: self.size,
            s: self.s,
        };
        self.job_sig = self.want_sig;
        if let Ok(mut j) = JOB.lock() {
            *j = Some(job);
        }
        JOB_CV.notify_one();
    }

    fn ready(&mut self) {
        // one atomic hand-off; a stale generation is dropped whole
        let payload = READY.lock().map(|mut s| s.take()).ok().flatten();
        let Some(payload) = payload else { return };
        if payload.gen != self.pending_gen {
            return; // a newer target or a hide superseded this grab
        }
        let Some(res) = payload.thumbs else { self.end(); return };
        self.icon = payload.icon;
        self.glass = payload.glass;
        if let Some(g) = &self.glass {
            self.cached = Some((glass_key(self.pos, self.size), g.clone()));
        }
        self.thumbs = res
            .into_iter()
            .map(|(h, title, img)| Thumb { hwnd: h, title, img, x: 0.0, y: 0.0, w: 0.0, h: 0.0, placeholder: false })
            .collect();
        self.place();
        self.hover = None;
        self.close_hov = None;
        self.render();
    }

    fn drop_window(&mut self, hwnd: isize) {
        let before = self.thumbs.len();
        self.thumbs.retain(|t| t.hwnd != hwnd);
        if self.thumbs.len() == before || self.thumbs.is_empty() {
            self.end();
            return;
        }
        if let Some(req) = self.req.as_mut() {
            req.wins.retain(|&w| w != hwnd);
        }
        self.place();
        self.hover = None;
        self.close_hov = None;
        self.render();
    }

    fn end(&mut self) {
        self.shown = false;
        self.pending_gen = u64::MAX;
        self.thumbs.clear();
        self.icon = None;
        self.glass = None;
        self.surface = None;
        self.hover = None;
        self.close_hov = None;
        self.idle = 0;
        self.alpha = 0.0;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn poll(&mut self) {
        if !self.shown {
            return; // nothing to police while hidden: skip the cursor work entirely
        }
        let mut p = POINT::default();
        unsafe {
            if GetCursorPos(&mut p).is_err() {
                return;
            }
        }
        let (px, py) = (p.x, p.y);
        let in_panel = px >= self.pos.0 && px < self.pos.0 + self.size.0 as i32 && py >= self.pos.1 && py < self.pos.1 + self.size.1 as i32;
        let in_dock = self.req.as_ref().map(|r| px >= r.dock.0 && px < r.dock.2 && py >= r.dock.1 && py < r.dock.3).unwrap_or(false);
        if in_panel || in_dock {
            self.idle = 0;
        } else {
            self.idle += POLL_MS as u128;
            if self.idle > HIDE_MS {
                self.end();
                return;
            }
        }
        if self.shown && self.want_sig != self.job_sig {
            let due = self.deadline.is_none_or(|d| Instant::now() >= d);
            if due {
                self.enqueue_job();
            }
        }
        if self.shown && self.alpha < 255.0 {
            self.alpha = (self.alpha + 255.0 * POLL_MS as f32 / FADE_MS).min(255.0);
            self.render();
        }
    }
}

thread_local! {
    static STATE: std::cell::RefCell<Option<Preview>> = const { std::cell::RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Preview) -> R) -> Option<R> {
    STATE.with(|st| st.borrow_mut().as_mut().map(f))
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PV_SHOW => {
            with(|p| p.show());
            return LRESULT(0);
        }
        WM_PV_READY => {
            with(|p| p.ready());
            return LRESULT(0);
        }
        WM_PV_HIDE => {
            with(|p| p.end());
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == TIMER_POLL => {
            with(|p| p.poll());
            return LRESULT(0);
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
            with(|p| {
                let mut hover = None;
                let mut close_hov = None;
                for (i, t) in p.thumbs.iter().enumerate() {
                    if x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h {
                        hover = Some(i);
                    }
                    let c = ORB * p.s;
                    let (cx, cy) = (t.x + t.w - c - 7.0 * p.s, t.y + 7.0 * p.s);
                    if x >= cx && x < cx + c && y >= cy && y < cy + c {
                        close_hov = Some(i);
                    }
                }
                if hover != p.hover || close_hov != p.close_hov {
                    p.hover = hover;
                    p.close_hov = close_hov;
                    p.render();
                } else {
                    p.hover = hover;
                    p.close_hov = close_hov;
                }
            });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with(|p| {
                if let Some(i) = p.close_hov {
                    let h = p.thumbs.get(i).map(|t| t.hwnd);
                    if let Some(h) = h {
                        p.close_hov = None;
                        p.drop_window(h);
                        shell::close(HWND(h as *mut _));
                    }
                    return;
                }
                if let Some(i) = p.hover {
                    let h = p.thumbs.get(i).map(|t| t.hwnd);
                    p.end();
                    if let Some(h) = h {
                        shell::activate(HWND(h as *mut _));
                    }
                }
            });
            return LRESULT(0);
        }
        WM_MBUTTONUP => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
            with(|p| {
                if let Some(i) = p.thumbs.iter().position(|t| x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h) {
                    let h = p.thumbs[i].hwnd;
                    p.drop_window(h);
                    shell::close(HWND(h as *mut _));
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(h, msg, wp, lp)
}

/// Start the preview thread: its window and a message loop.
pub fn start() {
    std::thread::spawn(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("WinMacPreview"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            w!("WinMacPreview"), w!("WIN MAC Preview"), WS_POPUP, 0, 0, 1, 1, HWND::default(), HMENU::default(), hinst, None,
        ) else {
            return;
        };
        HWND_PV.store(hwnd.0 as isize, Ordering::Relaxed);
        let dpi = GetDpiForWindow(hwnd);
        let pv = Preview {
            hwnd,
            req: None,
            thumbs: Vec::new(),
            icon: None,
            shown: false,
            surface: None,
            glass: None,
            s: dpi as f32 / 96.0,
            pos: (0, 0),
            size: (1, 1),
            hover: None,
            close_hov: None,
            idle: 0,
            pending_gen: 0,
            alpha: 0.0,
            sig: 0,
            want_sig: 0,
            job_sig: u64::MAX,
            deadline: None,
            cached: None,
        };
        STATE.with(|st| *st.borrow_mut() = Some(pv));
        std::thread::spawn(worker_loop);
        let _ = SetTimer(hwnd, TIMER_POLL, POLL_MS, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// Tell the preview to show this set of windows above the given dock anchor.
pub fn show(req: ShowReq) {
    if let Ok(mut g) = SHOW_REQ.lock() {
        *g = Some(req);
    }
    let h = HWND_PV.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as *mut _), WM_PV_SHOW, WPARAM(0), LPARAM(0));
        }
    }
}

/// Hide any visible preview.
pub fn hide() {
    let h = HWND_PV.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as *mut _), WM_PV_HIDE, WPARAM(0), LPARAM(0));
        }
    }
}
