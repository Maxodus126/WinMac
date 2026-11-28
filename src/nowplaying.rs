//! Watches the system media session and exposes the current track so the menu bar can draw a small
//! now-playing indicator next to Help. (The earlier floating pill was moved into the menu bar on request.)

use std::sync::Mutex;

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Playback,
};

static LATEST: Mutex<Option<(String, String, bool)>> = Mutex::new(None);

fn session() -> Option<Session> {
    Manager::RequestAsync().and_then(|o| o.get()).ok()?.GetCurrentSession().ok()
}

fn read_media() -> Option<(String, String, bool)> {
    let s = session()?;
    let p = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = p.Title().ok()?.to_string();
    if title.is_empty() {
        return None;
    }
    let artist = p.Artist().map(|a| a.to_string()).unwrap_or_default();
    let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|x| x == Playback::Playing);
    Some((title, artist, playing))
}

/// The current track, if the session reports one: (title, artist, playing).
pub fn current() -> Option<(String, String, bool)> {
    LATEST.lock().map(|g| g.clone()).ok().flatten()
}

/// Toggle play/pause on the current session (on its own thread; WinRT needs COM).
pub fn toggle() {
    std::thread::spawn(move || {
        let _com = crate::ComInit::new(windows::Win32::System::Com::COINIT_MULTITHREADED);
        if let Some(s) = session() {
            let _ = s.TryTogglePlayPauseAsync().and_then(|o| o.get());
        }
    });
}

pub fn prev() {
    std::thread::spawn(move || {
        let _com = crate::ComInit::new(windows::Win32::System::Com::COINIT_MULTITHREADED);
        if let Some(s) = session() {
            let _ = s.TrySkipPreviousAsync().and_then(|o| o.get());
        }
    });
}

pub fn next() {
    std::thread::spawn(move || {
        let _com = crate::ComInit::new(windows::Win32::System::Com::COINIT_MULTITHREADED);
        if let Some(s) = session() {
            let _ = s.TrySkipNextAsync().and_then(|o| o.get());
        }
    });
}

/// Bring the app that owns the media session to the foreground (clicking the now-playing text).
pub fn focus() {
    std::thread::spawn(move || {
        let _com = crate::ComInit::new(windows::Win32::System::Com::COINIT_MULTITHREADED);
        let Some(s) = session() else { return };
        let id = s.SourceAppUserModelId().map(|a| a.to_string()).unwrap_or_default();
        if id.is_empty() {
            return;
        }
        let mut sh = crate::shell::Shell::default();
        for w in sh.app_windows() {
            if w.aumid.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&id)) {
                crate::shell::activate(w.hwnd);
                return;
            }
        }
    });
}

pub fn start() {
    std::thread::spawn(|| {
        let _com = crate::ComInit::new(windows::Win32::System::Com::COINIT_MULTITHREADED);
        loop {
            let m = read_media();
            if let Ok(mut g) = LATEST.lock() {
                *g = m;
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
    });
}
