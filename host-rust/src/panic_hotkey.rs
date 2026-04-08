#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use tokio::sync::watch;
#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

#[cfg(windows)]
const HOTKEY_ID: i32 = 0x7273;
#[cfg(windows)]
const PANIC_BLOCK_SECS: u64 = 10;

#[cfg(windows)]
pub fn start_panic_hotkey_thread(panic_tx: watch::Sender<u64>, blocked_until_epoch_secs: Arc<AtomicU64>) {
    thread::spawn(move || unsafe {
        let mods = MOD_CONTROL | MOD_ALT | MOD_SHIFT;
        let vk_p = b'P' as u32;
        let registered = RegisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ID, mods, vk_p).is_ok();
        if !registered {
            eprintln!("[panic] failed to register global hotkey Ctrl+Alt+Shift+P");
            return;
        }

        let mut generation = 0u64;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).0 > 0 {
            if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_ID {
                generation = generation.wrapping_add(1);
                let _ = panic_tx.send(generation);
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_secs();
                blocked_until_epoch_secs.store(now + PANIC_BLOCK_SECS, Ordering::SeqCst);
                eprintln!("[panic] session terminated by user hotkey");
            }
        }

        let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), HOTKEY_ID);
    });
}
