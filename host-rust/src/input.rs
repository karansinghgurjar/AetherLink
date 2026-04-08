#[cfg(windows)]
use anyhow::{anyhow, Result};
#[cfg(windows)]
use crate::screen_capture;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

#[cfg(windows)]
pub fn move_mouse(x: i32, y: i32) -> Result<()> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if width <= 1 || height <= 1 {
        return Err(anyhow!("Invalid screen size: {}x{}", width, height));
    }

    let x_clamped = x.clamp(0, width - 1) as i64;
    let y_clamped = y.clamp(0, height - 1) as i64;
    let dx = ((x_clamped * 65535) / (width as i64 - 1)) as i32;
    let dy = ((y_clamped * 65535) / (height as i64 - 1)) as i32;

    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    send_inputs(&[input])
}

#[cfg(windows)]
pub fn move_mouse_normalized(rel_x: f64, rel_y: f64) -> Result<()> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if width <= 1 || height <= 1 {
        return Err(anyhow!("Invalid screen size: {}x{}", width, height));
    }

    let x = (rel_x.clamp(0.0, 1.0) * (width - 1) as f64).round() as i32;
    let y = (rel_y.clamp(0.0, 1.0) * (height - 1) as f64).round() as i32;
    move_mouse(x, y)
}

#[cfg(windows)]
pub fn move_mouse_normalized_on_monitor(rel_x: f64, rel_y: f64, monitor_index: u32) -> Result<()> {
    let bounds = screen_capture::monitor_bounds_for_index(monitor_index)?;
    if bounds.width <= 1 || bounds.height <= 1 {
        return Err(anyhow!("Invalid monitor size: {}x{}", bounds.width, bounds.height));
    }

    let x = bounds.left + (rel_x.clamp(0.0, 1.0) * (bounds.width - 1) as f64).round() as i32;
    let y = bounds.top + (rel_y.clamp(0.0, 1.0) * (bounds.height - 1) as f64).round() as i32;
    move_mouse(x, y)
}

#[cfg(windows)]
pub fn normalized_to_monitor_point(rel_x: f64, rel_y: f64, monitor_index: u32) -> Result<(i32, i32)> {
    let bounds = screen_capture::monitor_bounds_for_index(monitor_index)?;
    if bounds.width <= 1 || bounds.height <= 1 {
        return Err(anyhow!("Invalid monitor size: {}x{}", bounds.width, bounds.height));
    }

    let x = bounds.left + (rel_x.clamp(0.0, 1.0) * (bounds.width - 1) as f64).round() as i32;
    let y = bounds.top + (rel_y.clamp(0.0, 1.0) * (bounds.height - 1) as f64).round() as i32;
    Ok((x, y))
}

#[cfg(windows)]
pub fn left_click() -> Result<()> {
    let down = mouse_button_input(MOUSEEVENTF_LEFTDOWN);
    let up = mouse_button_input(MOUSEEVENTF_LEFTUP);
    send_inputs(&[down])?;
    std::thread::sleep(std::time::Duration::from_millis(15));
    send_inputs(&[up])
}

#[cfg(windows)]
pub fn right_click() -> Result<()> {
    let down = mouse_button_input(MOUSEEVENTF_RIGHTDOWN);
    let up = mouse_button_input(MOUSEEVENTF_RIGHTUP);
    send_inputs(&[down])?;
    std::thread::sleep(std::time::Duration::from_millis(15));
    send_inputs(&[up])
}

#[cfg(windows)]
pub fn mouse_scroll(delta: i32) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    send_inputs(&[input])
}

#[cfg(windows)]
pub fn send_key_down(vk: u16) -> Result<()> {
    let input = key_input(vk, false);
    send_inputs(&[input])
}

#[cfg(windows)]
pub fn send_key_up(vk: u16) -> Result<()> {
    let input = key_input(vk, true);
    send_inputs(&[input])
}

#[cfg(windows)]
fn mouse_button_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(windows)]
fn key_input(vk: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(windows)]
fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(anyhow!("SendInput sent {sent} of {} input events", inputs.len()));
    }
    Ok(())
}
