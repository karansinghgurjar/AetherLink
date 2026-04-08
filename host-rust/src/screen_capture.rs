#[cfg(windows)]
use anyhow::{anyhow, Context, Result};
#[cfg(windows)]
use image::codecs::jpeg::JpegEncoder;
#[cfg(windows)]
use image::{imageops::FilterType, DynamicImage, ImageBuffer, Rgba};
#[cfg(windows)]
use std::io::Cursor;
#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC,
    GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, SRCCOPY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

#[cfg(windows)]
use crate::session_config::SessionConfig;

#[cfg(windows)]
struct OwnedDc {
    hwnd: HWND,
    hdc: windows::Win32::Graphics::Gdi::HDC,
}

#[cfg(windows)]
impl Drop for OwnedDc {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseDC(self.hwnd, self.hdc);
        }
    }
}

#[cfg(windows)]
struct MemDc(windows::Win32::Graphics::Gdi::HDC);

#[cfg(windows)]
impl Drop for MemDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

#[cfg(windows)]
struct Bitmap(HBITMAP);

#[cfg(windows)]
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0.0));
        }
    }
}

#[cfg(windows)]
struct SelectGuard {
    dc: windows::Win32::Graphics::Gdi::HDC,
    old_obj: HGDIOBJ,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
pub struct MonitorBounds {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

#[cfg(windows)]
impl Drop for SelectGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.old_obj);
        }
    }
}

#[cfg(windows)]
pub fn capture_desktop_to_jpeg_bytes() -> Result<Vec<u8>> {
    capture_desktop_to_jpeg_bytes_with_config(&SessionConfig::default())
}

#[cfg(windows)]
pub fn capture_desktop_rgba_with_config(config: &SessionConfig) -> Result<(u32, u32, Vec<u8>)> {
    let monitor = monitor_bounds_for_index(config.monitor_index)?;
    let (width, height, rgba) = capture_monitor_rgba(monitor)?;
    maybe_resize_rgba(width, height, rgba, config.target_width)
}

#[cfg(windows)]
pub fn capture_desktop_to_jpeg_bytes_with_config(config: &SessionConfig) -> Result<Vec<u8>> {
    let (width, height, rgba) = capture_desktop_rgba_with_config(config)?;
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow!("Failed to build image buffer"))?;

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut cursor, config.jpeg_quality.clamp(1, 100));
        encoder
            .encode_image(&DynamicImage::ImageRgba8(image))
            .context("Failed to encode screenshot as JPEG")?;
    }

    Ok(cursor.into_inner())
}

#[cfg(windows)]
pub fn capture_and_save_desktop(path: &str) -> Result<()> {
    let (width, height, rgba) = capture_monitor_rgba(primary_monitor_bounds()?)?;
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow!("Failed to build image buffer"))?;

    DynamicImage::ImageRgba8(image)
        .save(path)
        .with_context(|| format!("Failed to save screenshot to {path}"))?;

    Ok(())
}

#[cfg(windows)]
pub fn list_monitors() -> Result<Vec<MonitorBounds>> {
    unsafe extern "system" fn enum_monitor_proc(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitors = &mut *(lparam.0 as *mut Vec<MonitorBounds>);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info as *mut MONITORINFO).as_bool() {
            let rect = info.rcMonitor;
            monitors.push(MonitorBounds {
                left: rect.left,
                top: rect.top,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
            });
        }
        true.into()
    }

    unsafe {
        let mut monitors = Vec::new();
        let ok = EnumDisplayMonitors(
            HDC(std::ptr::null_mut()),
            None,
            Some(enum_monitor_proc),
            LPARAM((&mut monitors as *mut Vec<MonitorBounds>) as isize),
        );
        if !ok.as_bool() {
            return Err(anyhow!("EnumDisplayMonitors failed"));
        }

        if monitors.is_empty() {
            monitors.push(primary_monitor_bounds()?);
        }

        Ok(monitors)
    }
}

#[cfg(windows)]
pub fn monitor_bounds_for_index(index: u32) -> Result<MonitorBounds> {
    let monitors = list_monitors()?;
    monitors
        .get(index as usize)
        .copied()
        .or_else(|| monitors.first().copied())
        .ok_or_else(|| anyhow!("No monitors found"))
}

#[cfg(windows)]
fn primary_monitor_bounds() -> Result<MonitorBounds> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(anyhow!("Invalid screen size: {}x{}", width, height));
    }
    Ok(MonitorBounds {
        left: 0,
        top: 0,
        width,
        height,
    })
}

#[cfg(windows)]
fn maybe_resize_rgba(width: u32, height: u32, rgba: Vec<u8>, target_width: Option<u32>) -> Result<(u32, u32, Vec<u8>)> {
    let image_buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow!("Failed to build image buffer"))?;

    let mut image = DynamicImage::ImageRgba8(image_buf);
    if let Some(target_width) = target_width {
        if target_width > 0 && target_width < image.width() {
            let src_w = image.width() as f32;
            let src_h = image.height() as f32;
            let target_h = ((target_width as f32 / src_w) * src_h).max(1.0).round() as u32;
            image = image.resize(target_width, target_h, FilterType::Lanczos3);
        }
    }

    let rgba = image.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

#[cfg(windows)]
fn capture_monitor_rgba(bounds: MonitorBounds) -> Result<(u32, u32, Vec<u8>)> {
    unsafe {
        let hwnd = HWND(std::ptr::null_mut());
        let screen_hdc = GetDC(hwnd);
        if screen_hdc.0.is_null() {
            return Err(anyhow!("GetDC failed"));
        }
        let screen_dc = OwnedDc {
            hwnd,
            hdc: screen_hdc,
        };

        let width = bounds.width;
        let height = bounds.height;
        if width <= 0 || height <= 0 {
            return Err(anyhow!("Invalid screen size: {}x{}", width, height));
        }

        let mem_hdc = CreateCompatibleDC(screen_dc.hdc);
        if mem_hdc.0.is_null() {
            return Err(anyhow!("CreateCompatibleDC failed"));
        }
        let mem_dc = MemDc(mem_hdc);

        let bitmap = CreateCompatibleBitmap(screen_dc.hdc, width, height);
        if bitmap.0.is_null() {
            return Err(anyhow!("CreateCompatibleBitmap failed"));
        }
        let bitmap = Bitmap(bitmap);

        let old_obj = SelectObject(mem_dc.0, HGDIOBJ(bitmap.0.0));
        if old_obj.0.is_null() {
            return Err(anyhow!("SelectObject failed"));
        }
        let _select_guard = SelectGuard {
            dc: mem_dc.0,
            old_obj,
        };

        BitBlt(mem_dc.0, 0, 0, width, height, screen_dc.hdc, bounds.left, bounds.top, SRCCOPY)
            .context("BitBlt failed")?;

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default(); 1],
        };

        let width_u32 = width as u32;
        let height_u32 = height as u32;
        let mut bgra = vec![0u8; (width_u32 * height_u32 * 4) as usize];

        let scanlines = GetDIBits(
            mem_dc.0,
            bitmap.0,
            0,
            height_u32,
            Some(bgra.as_mut_ptr() as *mut std::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if scanlines == 0 {
            return Err(anyhow!("GetDIBits failed"));
        }

        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        Ok((width_u32, height_u32, bgra))
    }
}
