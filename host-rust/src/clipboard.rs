#[cfg(windows)]
use anyhow::{anyhow, Context, Result};
#[cfg(windows)]
use std::ptr::copy_nonoverlapping;
#[cfg(windows)]
use std::sync::mpsc as std_mpsc;
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use tokio::sync::mpsc::UnboundedSender;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, RemoveClipboardFormatListener, SetClipboardData,
};
#[cfg(windows)]
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowLongPtrW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, TranslateMessage, CREATESTRUCTW,
    GWLP_USERDATA, HMENU, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE,
    WM_NCCREATE, WM_NCDESTROY, WNDCLASSW,
};

#[cfg(windows)]
const CF_UNICODETEXT_RAW: u32 = 13;
#[cfg(windows)]
const LISTENER_CLASS_NAME: &str = "AetherLinkClipboardListener";

#[cfg(windows)]
struct ClipboardGuard;

#[cfg(windows)]
struct ListenerWindowState {
    tx: UnboundedSender<()>,
}

#[cfg(windows)]
pub struct ClipboardListener {
    hwnd: HWND,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl Drop for ClipboardListener {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.0.is_null() {
                let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
impl ClipboardGuard {
    fn open() -> Result<Self> {
        unsafe {
            OpenClipboard(HWND(std::ptr::null_mut())).ok().context("OpenClipboard failed")?;
        }
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

#[cfg(windows)]
pub fn get_clipboard_text() -> Result<Option<String>> {
    let _guard = ClipboardGuard::open()?;
    unsafe {
        if !IsClipboardFormatAvailable(CF_UNICODETEXT_RAW).is_ok() {
            return Ok(None);
        }

        let handle = match GetClipboardData(CF_UNICODETEXT_RAW) {
            Ok(handle) if !handle.0.is_null() => handle,
            _ => return Ok(None),
        };

        let locked = GlobalLock(HGLOBAL(handle.0));
        if locked.is_null() {
            return Err(anyhow!("GlobalLock failed for clipboard data"));
        }

        let result = (|| {
            let ptr = locked.cast::<u16>();
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            String::from_utf16(slice).context("clipboard text is not valid UTF-16")
        })();

        let _ = GlobalUnlock(HGLOBAL(handle.0));
        result.map(Some)
    }
}

#[cfg(windows)]
pub fn set_clipboard_text(text: &str) -> Result<()> {
    let _guard = ClipboardGuard::open()?;
    unsafe {
        EmptyClipboard().ok().context("EmptyClipboard failed")?;

        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let size_bytes = wide.len() * std::mem::size_of::<u16>();
        let handle = GlobalAlloc(GMEM_MOVEABLE, size_bytes).context("GlobalAlloc failed for clipboard text")?;

        let locked = GlobalLock(handle);
        if locked.is_null() {
            let _ = GlobalFree(handle);
            return Err(anyhow!("GlobalLock failed for clipboard write"));
        }

        copy_nonoverlapping(wide.as_ptr().cast::<u8>(), locked.cast::<u8>(), size_bytes);
        let _ = GlobalUnlock(handle);

        if SetClipboardData(CF_UNICODETEXT_RAW, HANDLE(handle.0)).is_err() {
            let _ = GlobalFree(handle);
            return Err(anyhow!("SetClipboardData failed"));
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn start_clipboard_listener(tx: UnboundedSender<()>) -> Result<ClipboardListener> {
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<isize, String>>();
    let thread = thread::spawn(move || {
        let class_name: Vec<u16> = LISTENER_CLASS_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let wnd_class = WNDCLASSW {
                lpfnWndProc: Some(listener_wnd_proc),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&wnd_class);

            let state = Box::new(ListenerWindowState { tx });
            let state_ptr = Box::into_raw(state);
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                HMENU(std::ptr::null_mut()),
                HINSTANCE(std::ptr::null_mut()),
                Some(state_ptr.cast()),
            ) {
                Ok(hwnd) if !hwnd.0.is_null() => hwnd,
                _ => {
                    drop(Box::from_raw(state_ptr));
                    let _ = ready_tx.send(Err("CreateWindowExW failed".to_string()));
                    return;
                }
            };

            if let Err(err) = AddClipboardFormatListener(hwnd) {
                let _ = DestroyWindow(hwnd);
                let _ = ready_tx.send(Err(format!("AddClipboardFormatListener failed: {err}")));
                return;
            }

            let _ = ready_tx.send(Ok(hwnd.0 as isize));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
    });

    match ready_rx.recv().context("clipboard listener startup failed")? {
        Ok(hwnd) => Ok(ClipboardListener {
            hwnd: HWND(hwnd as *mut core::ffi::c_void),
            thread: Some(thread),
        }),
        Err(err) => {
            let _ = thread.join();
            Err(anyhow!(err))
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn listener_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            LRESULT(1)
        }
        WM_CLIPBOARDUPDATE => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ListenerWindowState;
            if !state_ptr.is_null() {
                let _ = (*state_ptr).tx.send(());
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let _ = RemoveClipboardFormatListener(hwnd);
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ListenerWindowState;
            if !state_ptr.is_null() {
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(state_ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
