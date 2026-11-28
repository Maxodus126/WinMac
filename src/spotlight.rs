//! Spotlight, as on macOS 26: a Liquid Glass capsule in the upper part of the screen that grows downward
//! with the results as you type. Apps first (the same list the Apps window shows), then a web search.
//! Opens from the menu bar's magnifier or Alt+Space.

use std::borrow::Cow;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::flyout::{white, Cmd, Content, Ui, ACCENT, MARGIN};
use crate::i18n::t;
use crate::launchpad::{ease_out, rank, refresh, App, BLINK_MS, CATALOG, NOTIFY, RESULTS_IN_MS};

const W: f32 = 660.0;
const PAD: (f32, f32) = (16.0, 0.0);
/// the capsule: its height is twice the glass corner radius, so the ends are round
const BAR: f32 = 52.0;
const ROW: f32 = 40.0;
const MAX_APPS: usize = 7;
const MAX_FILES: usize = 5;
const MAX_SETTINGS: usize = 6;
const LIST_TOP: f32 = BAR + 7.0;
const LIST_PAD: f32 = 9.0;
/// the tallest it gets: every app row, file rows, the calculator row and the web row
const MAX_H: f32 = LIST_TOP + (MAX_APPS + MAX_FILES + MAX_SETTINGS + 2) as f32 * ROW + LIST_PAD;
/// where the capsule's top sits, as a fraction of the screen height (macOS keeps it above the middle)
const TOP: f32 = 0.22;

const ID_FIELD: u32 = 1;
const ID_ROW: u32 = 10;

/// Tests disable real filesystem search so row counts stay deterministic.
static NO_FS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A file/folder search hit.
#[derive(Clone)]
struct FileHit {
    path: String,
    name: String,
    is_dir: bool,
}

/// A Windows settings/tool search hit: display name + what to launch (`ms-settings:…` URI, `.msc`,
/// `.exe`, or a Control Panel applet).
#[derive(Clone)]
struct SettingsHit {
    name: String,
    launch: String,
}

/// A curated subset of Windows settings and tools, each with its launcher.
const SETTINGS: &[(&str, &str)] = &[
    ("Display", "ms-settings:display"),
    ("Wi-Fi", "ms-settings:network-wifi"),
    ("Network", "ms-settings:network"),
    ("Bluetooth", "ms-settings:bluetooth"),
    ("Sound", "ms-settings:sound"),
    ("Add or remove programs", "ms-settings:appsfeatures"),
    ("Default apps", "ms-settings:defaultapps"),
    ("Personalization", "ms-settings:personalization"),
    ("Taskbar", "ms-settings:taskbar"),
    ("Notifications", "ms-settings:notifications"),
    ("Power & battery", "ms-settings:power"),
    ("Mouse", "ms-settings:mousetouchpad"),
    ("Keyboard", "ms-settings:keyboard"),
    ("Accounts", "ms-settings:accounts"),
    ("Privacy", "ms-settings:privacy"),
    ("Windows Update", "ms-settings:windowsupdate"),
    ("Device Manager", "devmgmt.msc"),
    ("Services", "services.msc"),
    ("Computer Management", "compmgmt.msc"),
    ("Disk Management", "diskmgmt.msc"),
    ("Task Manager", "taskmgr.exe"),
    ("Control Panel", "control.exe"),
    ("Command Prompt", "cmd.exe"),
    ("PowerShell", "powershell.exe"),
];

pub struct Spotlight {
    apps: Vec<App>,
    query: String,
    /// the apps matching, best first (indexes into `apps`)
    hits: Vec<usize>,
    /// file/folder matches for the current query
    files: Vec<FileHit>,
    /// Windows settings/tool matches for the current query
    settings: Vec<SettingsHit>,
    /// a calculator result for the current query (pure arithmetic expressions)
    calc: Option<String>,
    sel: usize,
    /// the content height on screen, easing to `want_h()`
    h: f32,
    blink: Instant,
    changed: Option<Instant>,
    last_tick: Option<Instant>,
    motion: bool,
    /// filesystem search runs off the UI thread; the worker drops (query, results) here
    files_job: Arc<Mutex<Option<(String, Vec<FileHit>)>>>,
    /// the query whose files we are waiting for (None = no search pending)
    files_pending: Option<String>,
    /// hwnd of the flyout window, so the worker can post WM_FLY_DATA when results land
    hwnd: isize,
}

/// %-encode a search for a URL.
fn encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Resolve a known folder GUID to a path, if it exists.
fn known_folder(id: &windows_core::GUID) -> Option<String> {
    unsafe {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KNOWN_FOLDER_FLAG};
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const std::ffi::c_void));
        s.filter(|s| std::path::Path::new(s).is_dir())
    }
}

/// A bounded search of the user's Desktop / Documents / Downloads (and the Start Menu) for files and
/// folders whose name contains `q`. Kept cheap: capped number of entries scanned and results returned.
fn find_files(q: &str) -> Vec<FileHit> {
    if q.len() < 2 {
        return Vec::new();
    }
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads};
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for id in [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads] {
        if let Some(p) = known_folder(&id) {
            roots.push(std::path::PathBuf::from(p));
        }
    }
    // App shortcuts live here; useful when the app list misses a just-installed tool.
    if let Some(p) = known_folder(&windows::Win32::UI::Shell::FOLDERID_Programs) {
        roots.push(std::path::PathBuf::from(p));
    }
    let mut out = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, usize)> = roots.into_iter().map(|p| (p, 0)).collect();
    let mut scanned = 0usize;
    const MAX_SCAN: usize = 2500;
    while let Some((dir, depth)) = stack.pop() {
        if scanned >= MAX_SCAN || out.len() >= MAX_FILES {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            if out.len() >= MAX_FILES || scanned >= MAX_SCAN {
                break;
            }
            scanned += 1;
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().into_owned();
            let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && depth < 3 && scanned < MAX_SCAN {
                stack.push((path.clone(), depth + 1));
            }
            if rank(&name.to_lowercase(), q).is_some() {
                out.push(FileHit { path: path.to_string_lossy().into_owned(), name, is_dir });
            }
        }
    }
    out
}

/// A tiny recursive-descent evaluator for arithmetic (+, -, *, /, %, ^, parentheses, decimals). Never
/// `eval`s anything; returns None if the whole trimmed query is not a valid expression.
fn eval_calc(q: &str) -> Option<String> {
    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }
    impl<'a> P<'a> {
        fn skip(&mut self) {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn peek(&mut self) -> Option<char> {
            self.skip();
            self.b.get(self.i).map(|&c| c as char)
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            loop {
                match self.peek() {
                    Some('+') => {
                        self.i += 1;
                        v += self.term()?;
                    }
                    Some('-') => {
                        self.i += 1;
                        v -= self.term()?;
                    }
                    _ => break,
                }
            }
            Some(v)
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.unary()?;
            loop {
                match self.peek() {
                    Some('*') => {
                        self.i += 1;
                        v *= self.unary()?;
                    }
                    Some('/') => {
                        self.i += 1;
                        let d = self.unary()?;
                        if d.abs() < 1e-12 {
                            return None;
                        }
                        v /= d;
                    }
                    Some('%') => {
                        self.i += 1;
                        let d = self.unary()?;
                        if d.abs() < 1e-12 {
                            return None;
                        }
                        v %= d;
                    }
                    _ => break,
                }
            }
            Some(v)
        }
        fn unary(&mut self) -> Option<f64> {
            match self.peek() {
                Some('-') => {
                    self.i += 1;
                    Some(-self.factor()?)
                }
                Some('+') => {
                    self.i += 1;
                    self.factor()
                }
                _ => self.factor(),
            }
        }
        fn factor(&mut self) -> Option<f64> {
            let base = self.primary()?;
            if self.peek() == Some('^') {
                self.i += 1;
                let e = self.factor()?; // right-associative
                Some(base.powf(e))
            } else {
                Some(base)
            }
        }
        fn primary(&mut self) -> Option<f64> {
            match self.peek() {
                Some('(') => {
                    self.i += 1;
                    let v = self.expr()?;
                    if self.peek() != Some(')') {
                        return None;
                    }
                    self.i += 1;
                    Some(v)
                }
                Some(_) => self.number(),
                None => None,
            }
        }
        fn number(&mut self) -> Option<f64> {
            self.skip();
            let start = self.i;
            let mut dot = false;
            while self.i < self.b.len() {
                match self.b[self.i] {
                    b'0'..=b'9' => self.i += 1,
                    b'.' if !dot => {
                        dot = true;
                        self.i += 1;
                    }
                    _ => break,
                }
            }
            if self.i == start {
                return None;
            }
            std::str::from_utf8(&self.b[start..self.i]).ok()?.parse::<f64>().ok()
        }
    }
    let s = q.trim();
    if s.is_empty() {
        return None;
    }
    let mut p = P { b: s.as_bytes(), i: 0 };
    let v = p.expr()?;
    if p.i != s.len() {
        return None; // leftover characters: not pure arithmetic
    }
    let txt = if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let mut t = format!("{:.10}", v);
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.pop();
        }
        t
    };
    Some(txt)
}

impl Spotlight {
    pub fn new(motion: bool) -> Spotlight {
        let mut s = Spotlight::empty(Vec::new(), motion);
        s.pick_up();
        refresh();
        s
    }

    fn empty(apps: Vec<App>, motion: bool) -> Spotlight {
        Spotlight { apps, query: String::new(), hits: Vec::new(), files: Vec::new(), settings: Vec::new(), calc: None, sel: 0, h: BAR, blink: Instant::now(), changed: None, last_tick: None, motion, files_job: Arc::new(Mutex::new(None)), files_pending: None, hwnd: 0 }
    }

    fn pick_up(&mut self) {
        self.apps = CATALOG.lock().map(|c| c.clone()).unwrap_or_default();
        self.filter();
    }

    fn filter(&mut self) {
        let q = self.query.trim().to_lowercase();
        self.hits.clear();
        self.calc = None;
        self.settings.clear();
        if !q.is_empty() {
            let mut hits: Vec<(u8, usize, usize)> =
                self.apps.iter().enumerate().filter_map(|(i, a)| rank(&a.name.to_lowercase(), &q).map(|r| (r, a.name.len(), i))).collect();
            hits.sort();
            self.hits = hits.into_iter().map(|h| h.2).take(MAX_APPS).collect();
            self.calc = eval_calc(&q);
            for &(name, launch) in SETTINGS {
                if name.to_lowercase().contains(&q) && self.settings.len() < MAX_SETTINGS {
                    self.settings.push(SettingsHit { name: name.to_string(), launch: launch.to_string() });
                }
            }
            // kick off an off-thread filesystem search; results land via data()
            if !NO_FS.load(Ordering::Relaxed) {
                if self.files_pending.as_deref() != Some(&q) {
                    let job = self.files_job.clone();
                    let q2 = q.clone();
                    let hwnd = self.hwnd;
                    std::thread::spawn(move || {
                        let res = find_files(&q2);
                        *job.lock().unwrap_or_else(|p| p.into_inner()) = Some((q2, res));
                        if hwnd != 0 {
                            unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                    windows::Win32::Foundation::HWND(hwnd as *mut _),
                                    crate::flyout::WM_FLY_DATA,
                                    windows::Win32::Foundation::WPARAM(0),
                                    windows::Win32::Foundation::LPARAM(0),
                                );
                            }
                        }
                    });
                }
                self.files_pending = Some(q.clone());
            } else {
                self.files_pending = None;
            }
        } else {
            self.files.clear();
            self.files_pending = None;
        }
        self.sel = self.sel.min(self.rows().saturating_sub(1));
    }

    /// Pick up results from a finished filesystem worker thread (called on WM_FLY_DATA / data()).
    fn pick_up_files(&mut self) {
        if let Some((q, res)) = self.files_job.lock().unwrap_or_else(|p| p.into_inner()).take() {
            if self.files_pending.as_deref() == Some(q.as_str()) && self.query.trim().to_lowercase() == q {
                self.files = res;
                self.sel = self.sel.min(self.rows().saturating_sub(1));
                self.files_pending = None;
            }
        }
    }

    /// Rows under the field: apps | files/folders | Windows settings/tools | calculator | web search
    /// (none while the field is empty).
    /// First row after apps+files (the settings band begins here).
    fn a_settings(&self) -> usize {
        self.hits.len() + self.files.len() + self.settings.len()
    }

    fn rows(&self) -> usize {
        if self.query.trim().is_empty() {
            0
        } else {
            self.a_settings() + if self.calc.is_some() { 1 } else { 0 } + 1
        }
    }

    /// Is `r` the web-search row (the last one)?
    fn is_web_row(&self, r: usize) -> bool {
        r == self.a_settings() + if self.calc.is_some() { 1 } else { 0 }
    }

    fn want_h(&self) -> f32 {
        match self.rows() {
            0 => BAR,
            n => LIST_TOP + n as f32 * ROW + LIST_PAD,
        }
    }

    fn set_query(&mut self, q: String) {
        self.query = q;
        self.sel = 0;
        self.blink = Instant::now();
        self.filter();
        if self.motion {
            self.changed = Some(Instant::now());
        } else {
            self.h = self.want_h();
        }
    }

    fn open(&self, row: usize) -> Cmd {
        let a = self.hits.len();
        let af = a + self.files.len();
        if let Some(&ai) = self.hits.get(row) {
            let spec = self.apps[ai].spec.clone();
            crate::apps::remember(&spec);
            Cmd::Launch(spec)
        } else if row < af {
            Cmd::Launch(self.files[row - a].path.clone())
        } else if row < self.a_settings() {
            Cmd::Launch(self.settings[row - af].launch.clone())
        } else if self.calc.is_some() && row == self.a_settings() {
            Cmd::None // a calculator result: nothing to launch
        } else if self.is_web_row(row) && !self.query.trim().is_empty() {
            Cmd::Launch(format!("https://www.google.com/search?q={}", encode(self.query.trim())))
        } else {
            Cmd::None
        }
    }

    fn draw_field(&mut self, ui: &mut Ui) {
        let cw = ui.w;
        let my = BAR / 2.0;
        // the magnifier
        let c = white(0xC8);
        ui.outline(2.0, my - 9.0, 14.0, 14.0, 7.0, 1.9, c);
        ui.line(13.6, my + 2.6, 18.5, my + 7.5, 2.1, c);
        ui.hit(ID_FIELD, 0.0, 0.0, cw, BAR);
        let tx = 30.0;
        let max = cw - tx - 8.0;
        let caret_on = (self.blink.elapsed().as_millis() / BLINK_MS) % 2 == 0;
        let caret = (ACCENT.0, ACCENT.1, ACCENT.2, 255);
        let end = if self.query.is_empty() {
            ui.text_mid(tx, my, t("Spotlight Search"), 21.0, 400, white(0x8C), max);
            0.0
        } else {
            ui.text_mid(tx, my, &self.query, 21.0, 400, white(0xF5), max)
        };
        if caret_on {
            ui.fill(tx + end.min(max) + if end > 0.0 { 1.0 } else { -1.5 }, my - 11.0, 1.6, 22.0, 0.0, caret);
        }
    }

    fn draw_rows(&mut self, ui: &mut Ui) {
        let cw = ui.w;
        let n = self.rows();
        if n == 0 || self.h <= BAR + 1.0 {
            return;
        }
        let a = match self.changed {
            Some(c) => ease_out(c.elapsed().as_secs_f32() * 1000.0 / RESULTS_IN_MS),
            None => 1.0,
        };
        let view = self.h - BAR;
        // Borrow instead of cloning per frame: draw_rows runs every frame while typing.
        let sel = self.sel;
        let q = self.query.trim();
        let hits = &self.hits;
        let apps = &self.apps;
        let files = &self.files;
        let settings = &self.settings;
        let calc = &self.calc;
        ui.layer(-PAD.0, BAR, cw + 2.0 * PAD.0, view, 1.0, 1.0, [0.0, 0.0, 0.0, 0.0], |ui| {
            ui.line(0.0, BAR + 0.5, cw, BAR + 0.5, 1.0, white(0x1F));
            let rise = (1.0 - a) * 6.0;
            for r in 0..n {
                let y = LIST_TOP + r as f32 * ROW + rise;
                let id = ID_ROW + r as u32;
                let hov = ui.hit(id, -8.0, y, cw + 16.0, ROW);
                let on = r == sel;
                if on {
                    ui.fill(-8.0, y + 1.0, cw + 16.0, ROW - 2.0, 11.0, (ACCENT.0, ACCENT.1, ACCENT.2, (0xC8 as f32 * a) as u8));
                } else if hov {
                    ui.fill(-8.0, y + 1.0, cw + 16.0, ROW - 2.0, 11.0, white((0x1A as f32 * a) as u8));
                }
                let ty = y + ROW / 2.0;
                let fade = |c: u8| (c as f32 * a) as u8;
                let a_hits = hits.len();
                let a_files = a_hits + files.len();
                let a_settings = a_files + settings.len();
                let (name, kind) = if r < a_hits {
                    let app = &apps[hits[r]];
                    match &app.icon {
                        Some(img) => ui.image(img, 0.0, ty - 14.0, 28.0, 28.0),
                        None => ui.fill(2.0, ty - 12.0, 24.0, 24.0, 6.0, white(0x22)),
                    }
                    (Cow::Borrowed(app.name.as_str()), t("Application"))
                } else if r < a_files {
                    let f = &files[r - a_hits];
                    if f.is_dir {
                        ui.glyph(14.0, ty, '\u{E75C}', 17.0, white(fade(0xE0)));
                    } else {
                        ui.glyph(14.0, ty, '\u{E73E}', 16.0, white(fade(0xE0)));
                    }
                    (Cow::Owned(f.name.clone()), t(if f.is_dir { "Folder" } else { "Document" }))
                } else if r < a_settings {
                    ui.glyph(14.0, ty, '\u{E8B7}', 17.0, white(fade(0xE0)));
                    (Cow::Owned(settings[r - a_files].name.clone()), t("Settings"))
                } else if calc.is_some() && r == a_settings {
                    ui.glyph(14.0, ty, '\u{E8EF}', 17.0, white(fade(0xE0)));
                    (Cow::Owned(format!("= {}", calc.as_ref().unwrap())), t("Calculator"))
                } else {
                    ui.glyph(14.0, ty, '\u{E774}', 17.0, white(fade(0xE0)));
                    (Cow::Owned(format!("{} \u{201C}{q}\u{201D}", t("Search the Web for"))), "")
                };
                let kw = if kind.is_empty() { 0.0 } else { ui.text_w(kind, 12.0, 400) + 12.0 };
                ui.text_mid(40.0, ty, &name, 14.5, if on { 500 } else { 400 }, white(fade(0xF5)), cw - 48.0 - kw);
                if !kind.is_empty() {
                    ui.text_mid(cw - kw + 4.0, ty, kind, 12.0, 400, white(fade(if on { 0xE0 } else { 0x8C })), 0.0);
                }
            }
        });
    }
}

impl Content for Spotlight {
    fn width(&self) -> f32 {
        W + 2.0 * MARGIN
    }

    fn padding(&self) -> (f32, f32) {
        PAD
    }

    fn glass(&self) -> Option<f32> {
        Some(self.h)
    }

    fn glass_top(&self) -> Option<(f32, f32)> {
        Some((TOP, MAX_H))
    }

    fn motion(&self) -> bool {
        self.motion
    }

    fn animating(&self) -> bool {
        (self.h - self.want_h()).abs() > 0.25 || self.changed.is_some()
    }

    fn period_ms(&self) -> u32 {
        BLINK_MS as u32 / 2
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = self.last_tick.map_or(0.016, |t| (now - t).as_secs_f32()).min(0.05);
        self.last_tick = Some(now);
        let goal = self.want_h();
        // a quick spring-like ease, the way the capsule opens into the list on macOS
        self.h = if !self.motion || (self.h - goal).abs() <= 0.25 { goal } else { goal + (self.h - goal) * (-dt * 18.0).exp() };
        if self.changed.is_some_and(|c| c.elapsed().as_secs_f32() * 1000.0 > RESULTS_IN_MS) {
            self.changed = None;
        }
    }

    fn opened(&mut self, hwnd: isize) {
        NOTIFY.store(hwnd, Ordering::Relaxed);
        self.hwnd = hwnd;
    }

    fn closing(&mut self) {
        NOTIFY.store(0, Ordering::Relaxed);
    }

    fn data(&mut self) {
        self.pick_up_files();
    }

    fn draw(&mut self, ui: &mut Ui) -> f32 {
        self.draw_field(ui);
        self.draw_rows(ui);
        self.h
    }

    fn hover(&mut self, id: Option<u32>) {
        if let Some(r) = id.and_then(|i| i.checked_sub(ID_ROW)) {
            self.sel = r as usize;
        }
    }

    fn click(&mut self, id: u32, _fx: f32) -> Cmd {
        match id.checked_sub(ID_ROW) {
            Some(r) => self.open(r as usize),
            None => {
                self.blink = Instant::now();
                Cmd::None
            }
        }
    }

    fn right_click(&mut self, id: u32) -> Cmd {
        match id.checked_sub(ID_ROW).and_then(|r| self.hits.get(r as usize)) {
            Some(&ai) => Cmd::Pin(self.apps[ai].spec.clone()),
            None => Cmd::None,
        }
    }

    fn char(&mut self, c: char) -> Cmd {
        match c {
            '\u{8}' => {
                let mut q = self.query.clone();
                q.pop();
                self.set_query(q);
            }
            '\r' => return self.open(self.sel),
            '\t' => {}
            c if !c.is_control() && self.query.chars().count() < 60 => {
                let q = format!("{}{c}", self.query);
                self.set_query(q);
            }
            _ => {}
        }
        Cmd::None
    }

    fn key(&mut self, vk: u32) -> Cmd {
        let n = self.rows();
        match vk {
            0x26 if n > 0 => self.sel = (self.sel + n - 1) % n,
            0x28 if n > 0 => self.sel = (self.sel + 1) % n,
            _ => {}
        }
        Cmd::None
    }

    fn escape(&mut self) -> Cmd {
        if self.query.is_empty() {
            Cmd::Close
        } else {
            self.set_query(String::new());
            Cmd::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn no_fs() {
        NO_FS.store(true, Ordering::Relaxed);
    }

    fn spot(names: &[&str]) -> Spotlight {
        let list = names.iter().map(|n| App { name: n.to_string(), spec: format!("shell:AppsFolder\\{n}"), icon: None }).collect();
        let mut s = Spotlight::empty(list, false);
        no_fs();
        s.filter();
        s
    }

    #[test]
    fn calc_evaluates_arithmetic() {
        assert_eq!(eval_calc("2+3*4"), Some("14".into()));
        assert_eq!(eval_calc("(2+3)^2"), Some("25".into()));
        assert_eq!(eval_calc("10%3"), Some("1".into()));
        assert_eq!(eval_calc("1.5*2"), Some("3".into()));
        assert_eq!(eval_calc("-3 + 7"), Some("4".into()));
        assert_eq!(eval_calc("1/0"), None);
        assert_eq!(eval_calc("2+"), None);
        assert_eq!(eval_calc("2x3"), None);
        assert_eq!(eval_calc("hello"), None);
        assert_eq!(eval_calc(""), None);
    }

    #[test]
    fn calc_row_surfaces_and_opens_nothing() {
        no_fs();
        let mut s = Spotlight::empty(Vec::new(), false);
        for c in "12+3".chars() {
            s.char(c);
        }
        assert_eq!(s.calc.as_deref(), Some("15"));
        // rows = apps(0) + files(0) + calc(1) + web(1)
        assert_eq!(s.rows(), 2);
        assert!(matches!(s.open(0), Cmd::None));
        assert!(matches!(s.open(1), Cmd::Launch(_)));
    }

    #[test]
    fn results_rank_and_end_with_the_web() {
        let mut s = spot(&["Visual Studio Code", "Code Writer", "Calculator", "Chrome"]);
        assert_eq!(s.rows(), 0);
        assert_eq!(s.want_h(), BAR);
        for c in "code".chars() {
            s.char(c);
        }
        let names: Vec<&str> = s.hits.iter().map(|&i| s.apps[i].name.as_str()).collect();
        assert_eq!(names, ["Code Writer", "Visual Studio Code"]);
        assert_eq!(s.rows(), 3);
        assert_eq!(s.h, s.want_h()); // no motion: it jumps
        s.key(0x26); // up from the first wraps to the web row
        match s.char('\r') {
            Cmd::Launch(u) => assert_eq!(u, "https://www.google.com/search?q=code"),
            _ => panic!("enter on the web row should search"),
        }
        assert!(matches!(s.escape(), Cmd::None));
        assert!(s.query.is_empty());
        assert!(matches!(s.escape(), Cmd::Close));
        assert_eq!(encode("a b&c"), "a+b%26c");
    }

    /// Spotlight with a search typed, on the real app list: target/spotlight.png
    #[test]
    #[ignore]
    fn spotlight_snapshot() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        crate::launchpad::load_now();
        let mut s = Spotlight::empty(Vec::new(), false);
        s.pick_up();
        for c in "co".chars() {
            s.char(c);
        }
        s.blink = Instant::now();
        let (w, h, px) = crate::flyout::snapshot_glass(&mut s, 1.25);
        let path = format!(r"{}\target\spotlight.png", env!("CARGO_MANIFEST_DIR"));
        assert!(crate::gfx::save_png(&path, w, h, &px));
    }
}
