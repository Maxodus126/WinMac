//! Hot Corners (macOS-style): when the pointer parks in a screen corner, fire an action once. Defaults:
//! top-left → Spotlight, top-right → Apps/Launchpad, bottom-right → show desktop, bottom-left → none.
//! Runs on a small thread of its own so the rest of the app can sleep; a debounce keeps a corner from
//! re-triggering while you pass through it.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::{push, Ev};

/// Pixels from the screen edge that count as "in the corner".
const CORNER_PX: i32 = 4;
/// Ignore retriggers within this window (so sweeping through a corner doesn't spam).
const DEBOUNCE_MS: u64 = 600;
const POLL_MS: u64 = 100;

pub fn start() {
    std::thread::spawn(|| {
        let mut last_fire = Instant::now() - Duration::from_millis(DEBOUNCE_MS);
        let mut active = [false; 4];
        loop {
            std::thread::sleep(Duration::from_millis(POLL_MS));
            let mut p = POINT::default();
            unsafe {
                if GetCursorPos(&mut p).is_err() {
                    continue;
                }
            }
            let sx = unsafe { GetSystemMetrics(SM_CXSCREEN) } as i32;
            let sy = unsafe { GetSystemMetrics(SM_CYSCREEN) } as i32;
            let idx = if p.x <= CORNER_PX && p.y <= CORNER_PX {
                Some(0) // top-left
            } else if p.x >= sx - CORNER_PX && p.y <= CORNER_PX {
                Some(1) // top-right
            } else if p.x <= CORNER_PX && p.y >= sy - CORNER_PX {
                Some(2) // bottom-left
            } else if p.x >= sx - CORNER_PX && p.y >= sy - CORNER_PX {
                Some(3) // bottom-right
            } else {
                None
            };
            if let Some(i) = idx {
                if !active[i] {
                    active = [false; 4];
                    active[i] = true;
                    if last_fire.elapsed().as_millis() >= DEBOUNCE_MS as u128 {
                        last_fire = Instant::now();
                        push(Ev::HotCorner(i as u8));
                    }
                }
            } else {
                active = [false; 4];
            }
        }
    });
}

/// Show the Windows desktop (Win+D).
pub fn show_desktop() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_LWIN,
    };
    fn ki(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    ..Default::default()
                },
            },
        }
    }
    unsafe {
        let _ = SendInput(&[ki(VK_LWIN.0 as u16, false), ki('D' as u16, false), ki('D' as u16, true), ki(VK_LWIN.0 as u16, true)], std::mem::size_of::<INPUT>() as i32);
    }
}
