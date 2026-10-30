//! Screen capture around the cursor with GDI `BitBlt`.
use image::RgbImage;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

pub struct Shot {
    pub image: RgbImage,
    /// Cursor inside `image`.
    pub mouse: (i32, i32),
    /// EFT UI scale: monitor height / 1080.
    pub scale: f32,
}

/// Captures the area ChadVision needs around screen point (`x`, `y`): a 896-px (×scale) square for
/// the cell, widened to the right to cover the hover tooltip window (700 px ×scale). Clamped to the
/// virtual screen, so nothing off-screen is captured.
pub fn around(x: i32, y: i32) -> anyhow::Result<Shot> {
    let (_, top, _, bottom) = super::monitor_rect(x, y);
    let scale = (bottom - top) as f32 / 1080.0;
    let half = (448.0 * scale).round() as i32;
    let right = (700.0 * scale).round().max(half as f32) as i32;
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let x0 = (x - half).max(vx);
    let y0 = (y - half).max(vy);
    let x1 = (x + right).min(vx + vw);
    let y1 = (y + half).min(vy + vh);
    let (w, h) = (x1 - x0, y1 - y0);
    anyhow::ensure!(w > 0 && h > 0, "cursor outside the virtual screen");

    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let result = (|| {
            let dib = dib?;
            let old = SelectObject(mem, dib);
            // No CAPTUREBLT: layered windows (our own card) are not captured.
            let blit = BitBlt(mem, 0, 0, w, h, screen, x0, y0, SRCCOPY);
            SelectObject(mem, old);
            let out = blit.map(|()| {
                let stride = ((w * 3 + 3) & !3) as usize;
                let src = std::slice::from_raw_parts(bits as *const u8, stride * h as usize);
                let mut buf = Vec::with_capacity((w * h * 3) as usize);
                for row in src.chunks_exact(stride) {
                    for px in row[..w as usize * 3].chunks_exact(3) {
                        buf.extend_from_slice(&[px[2], px[1], px[0]]);
                    }
                }
                RgbImage::from_raw(w as u32, h as u32, buf).expect("buffer size")
            });
            let _ = DeleteObject(dib);
            Ok::<_, windows::core::Error>(out?)
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        Ok(Shot { image: result?, mouse: (x - x0, y - y0), scale })
    }
}
