//! Dynamic Island: a slim Liquid Glass pill near the top centre that shows the currently playing media
//! with transport controls (a compact "now playing" glance). Opens from the menu bar's island item.

use std::sync::Mutex;

use tiny_skia::Pixmap;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Playback,
};
use windows::Storage::Streams::DataReader;

use crate::flyout::{white, Cmd, Content, Ui};

const W: f32 = 460.0;
const H: f32 = 84.0;
const TOP: f32 = 0.012;
const MAX_H: f32 = H;

const ID_PREV: u32 = 1;
const ID_PLAY: u32 = 2;
const ID_NEXT: u32 = 3;

struct Media {
    title: String,
    artist: String,
    playing: bool,
    art: Option<Pixmap>,
}

static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

fn session() -> Option<Session> {
    let mut m = MANAGER.lock().unwrap_or_else(|p| p.into_inner());
    if m.is_none() {
        *m = Manager::RequestAsync().and_then(|o| o.get()).ok();
    }
    m.as_ref()?.GetCurrentSession().ok()
}

fn read_media() -> Option<Media> {
    let s = session()?;
    let p = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = p.Title().ok()?.to_string();
    if title.is_empty() {
        return None;
    }
    let mut artist = p.Artist().map(|a| a.to_string()).unwrap_or_default();
    if artist.is_empty() {
        let id = s.SourceAppUserModelId().map(|a| a.to_string()).unwrap_or_default();
        artist = crate::shell::friendly_name(None, Some(&id), &id);
    }
    let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|x| x == Playback::Playing);
    let art = (|| -> Option<Pixmap> {
        let stream = p.Thumbnail().ok()?.OpenReadAsync().ok()?.get().ok()?;
        let size = stream.Size().ok()? as u32;
        if size == 0 || size > 8 << 20 {
            return None;
        }
        let r = DataReader::CreateDataReader(&stream).ok()?;
        r.LoadAsync(size).ok()?.get().ok()?;
        let mut buf = vec![0u8; size as usize];
        r.ReadBytes(&mut buf).ok()?;
        crate::gfx::image_bytes(&buf, 80)
    })();
    Some(Media { title, artist, playing, art })
}

pub struct DynamicIsland {
    media: Option<Media>,
    hwnd: isize,
}

impl DynamicIsland {
    pub fn new() -> DynamicIsland {
        DynamicIsland { media: None, hwnd: 0 }
    }

    fn media_cmd(&self, id: u32) {
        std::thread::spawn(move || {
            let _com = crate::ComInit::new(windows::Win32::System::Com::COINIT_MULTITHREADED);
            if let Some(s) = session() {
                let _ = match id {
                    ID_PREV => s.TrySkipPreviousAsync(),
                    ID_NEXT => s.TrySkipNextAsync(),
                    _ => s.TryTogglePlayPauseAsync(),
                }
                .and_then(|o| o.get());
            }
            // the panel's tick() (every 1.5s) repolls and refreshes the pill
        });
    }
}

impl Content for DynamicIsland {
    fn width(&self) -> f32 {
        W
    }

    fn glass_top(&self) -> Option<(f32, f32)> {
        Some((TOP, MAX_H))
    }

    fn period_ms(&self) -> u32 {
        1500
    }

    fn opened(&mut self, hwnd: isize) {
        self.hwnd = hwnd;
        self.media = read_media();
    }

    fn tick(&mut self) {
        if let Some(m) = read_media() {
            self.media = Some(m);
        }
    }

    fn click(&mut self, id: u32, _fx: f32) -> Cmd {
        match id {
            ID_PREV | ID_PLAY | ID_NEXT => {
                self.media_cmd(id);
                Cmd::None
            }
            _ => Cmd::None,
        }
    }

    fn draw(&mut self, ui: &mut Ui) -> f32 {
        let w = ui.w;
        let h = H;
        ui.fill(0.0, 0.0, w, h, (h / 2.0) as f32, white(0x14));
        ui.outline(0.0, 0.0, w, h, (h / 2.0) as f32, 1.0, white(0x14));
        let Some(m) = &self.media else {
            ui.text_mid(0.0, h / 2.0 - 7.0, crate::i18n::t("Nothing playing"), 14.0, 500, white(0x8C), w);
            return h;
        };
        // art
        let (ax, ay) = (12.0, (h - 52.0) / 2.0);
        ui.fill(ax, ay, 52.0, 52.0, 10.0, white(0x1E));
        if let Some(a) = &m.art {
            let side = a.width().min(a.height()) as f32;
            let (sw, sh) = (52.0 * a.width() as f32 / side, 52.0 * a.height() as f32 / side);
            ui.image_rounded(a, ax - (sw - 52.0) / 2.0, ay - (sh - 52.0) / 2.0, sw, sh, (ax, ay, 52.0, 52.0), 10.0);
        }
        let tx = ax + 52.0 + 14.0;
        let ctrl_w = 3.0 * 30.0;
        let tw = w - tx - ctrl_w - 14.0;
        let (h1, h2) = (ui.line_h(15.0, 600), ui.line_h(12.0, 400));
        let ty = (h - h1 - h2) / 2.0;
        ui.text(tx, ty, &m.title, 15.0, 600, white(0xF2), tw);
        ui.text(tx, ty + h1, &m.artist, 12.0, 400, white(0x99), tw);
        let mut bx = w - 14.0 - ctrl_w;
        for (id, g, size) in [
            (ID_PREV, '\u{E892}', 14.0),
            (ID_PLAY, if m.playing { '\u{E769}' } else { '\u{E768}' }, 16.0),
            (ID_NEXT, '\u{E893}', 14.0),
        ] {
            let by = (h - 30.0) / 2.0;
            if ui.hit(id, bx, by, 30.0, 30.0) {
                ui.fill(bx, by, 30.0, 30.0, 15.0, white(0x1E));
            }
            ui.glyph(bx + 15.0, by + 15.0, g, size, white(0xF2));
            bx += 30.0;
        }
        h
    }
}
