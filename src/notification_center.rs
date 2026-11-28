//! Notification Center: a Liquid Glass panel with a clock/date glance and a Recent Items list.
//! Appears centered like the other glass panels; the recent apps come from the saved config, so the
//! list matches what the Dock shows.

use std::time::Instant;

use crate::flyout::{white, Cmd, Content, Ui, MARGIN};
use crate::i18n::t;

const W: f32 = 340.0;
const HEADER: f32 = 46.0;
const DIVIDER: f32 = 1.0;
const ROW: f32 = 34.0;
const MAX_RECENT: usize = 6;
const PAD_Y: f32 = 12.0;

const ID_HEADER: u32 = 1;
const ID_ROW: u32 = 10;

struct Recent {
    name: String,
    launch: String,
}

pub struct NotificationCenter {
    recents: Vec<Recent>,
    now: Instant,
    blink: Instant,
}

fn load_recents() -> Vec<Recent> {
    let cfg = crate::config::Config::load();
    cfg.recent
        .iter()
        .filter_map(|v| {
            let name = v.get("Name").and_then(|n| n.as_str()).unwrap_or("");
            let launch = v
                .get("Launch")
                .and_then(|l| l.as_str())
                .or_else(|| v.get("Key").and_then(|k| k.as_str()))
                .unwrap_or("");
            if name.is_empty() || launch.is_empty() {
                None
            } else {
                Some(Recent { name: name.to_string(), launch: launch.to_string() })
            }
        })
        .take(MAX_RECENT)
        .collect()
}

/// Local time as (HH:MM, date).
fn local_now() -> (String, String) {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let hh = format!("{:02}", st.wHour);
    let mm = format!("{:02}", st.wMinute);
    let week = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][(st.wDayOfWeek as usize).min(6)];
    let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mon = months[(st.wMonth as usize).saturating_sub(1).min(11)];
    let date = format!("{week}, {mon} {:02}, {:04}", st.wDay, st.wYear);
    (format!("{hh}:{mm}"), date)
}

impl NotificationCenter {
    pub fn new() -> NotificationCenter {
        NotificationCenter { recents: load_recents(), now: Instant::now(), blink: Instant::now() }
    }

    fn h(&self) -> f32 {
        HEADER + DIVIDER + if self.recents.is_empty() { ROW } else { self.recents.len() as f32 * ROW } + PAD_Y * 2.0
    }

    fn draw_glance(&mut self, ui: &mut Ui) {
        let (time, date) = local_now();
        let y = 14.0;
        // big time
        let ty = y + 4.0;
        ui.text_mid(0.0, ty, &time, 34.0, 700, white(0xFA), ui.w);
        let dy = ty + 40.0;
        ui.text_mid(0.0, dy, &date, 13.0, 500, white(0xAE), ui.w);
        ui.hit(ID_HEADER, 0.0, 0.0, ui.w, HEADER);
    }

    fn draw_recents(&mut self, ui: &mut Ui) {
        let n = self.recents.len();
        if n == 0 {
            ui.text_mid(0.0, HEADER + DIVIDER + 20.0, t("No recent items"), 13.0, 400, white(0x8C), ui.w);
            ui.hit(ID_ROW, 0.0, HEADER + DIVIDER, ui.w, ROW);
            return;
        }
        for (i, r) in self.recents.iter().enumerate() {
            let y = HEADER + DIVIDER + i as f32 * ROW;
            let id = ID_ROW + i as u32;
            let hov = ui.hit(id, 0.0, y, ui.w, ROW);
            let on = false;
            if hov {
                ui.fill(6.0, y + 3.0, ui.w - 12.0, ROW - 6.0, 9.0, white(0x16));
            }
            let cy = y + ROW / 2.0;
            ui.fill(16.0, cy - 13.0, 26.0, 26.0, 7.0, white(0x22));
            ui.glyph(16.0 + 13.0, cy, '\u{E7F4}', 13.0, white(on.then_some(0xE0).unwrap_or(0x8C)));
            ui.text(52.0, cy - 7.0, &r.name, 13.5, 500, white(0xF2), ui.w - 60.0);
        }
    }
}

impl Content for NotificationCenter {
    fn width(&self) -> f32 {
        W + 2.0 * MARGIN
    }

    fn glass(&self) -> Option<f32> {
        Some(self.h())
    }

    fn period_ms(&self) -> u32 {
        1000
    }

    fn tick(&mut self) {
        self.now = Instant::now();
    }

    fn click(&mut self, id: u32, _fx: f32) -> Cmd {
        match id.checked_sub(ID_ROW) {
            Some(i) => self.recents.get(i as usize).map(|r| Cmd::Launch(r.launch.clone())).unwrap_or(Cmd::None),
            None => Cmd::None,
        }
    }

    fn draw(&mut self, ui: &mut Ui) -> f32 {
        self.draw_glance(ui);
        ui.line(14.0, HEADER, ui.w - 14.0, HEADER, DIVIDER, white(0x1F));
        self.draw_recents(ui);
        self.h()
    }
}
