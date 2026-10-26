//! The tray icon is drawn in code (a green leaf), so the exe needs no resource compiler.

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

pub fn leaf(size: i32) -> HICON {
    let size = size.clamp(16, 256);
    let pixels = render(size as usize);
    unsafe {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size;
        bmi.bmiHeader.biHeight = -size;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as u32;

        let mut bits = null_mut();
        let screen = GetDC(null_mut());
        let color = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ReleaseDC(null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u32, pixels.len());

        let mask_bytes = vec![0u8; (((size + 15) / 16) * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, mask_bytes.as_ptr() as _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

/// A leaf pointing to the top right: the overlap of two circles, with a light centre vein and a stem.
fn render(size: usize) -> Vec<u32> {
    const SAMPLES: usize = 4;
    const HALF_LENGTH: f32 = 0.86;
    const HALF_WIDTH: f32 = 0.40;
    const VEIN: f32 = 0.07;
    let offset = (HALF_LENGTH * HALF_LENGTH - HALF_WIDTH * HALF_WIDTH) / (2.0 * HALF_WIDTH);
    let radius_sq = (HALF_WIDTH + offset) * (HALF_WIDTH + offset);

    let leaf = (0x4C as f32, 0xAF as f32, 0x50 as f32);
    let vein = (0xC8 as f32, 0xE6 as f32, 0xC9 as f32);
    let stem = (0x38 as f32, 0x8E as f32, 0x3C as f32);

    let mut out = vec![0u32; size * size];
    for py in 0..size {
        for px in 0..size {
            let (mut hits, mut r, mut g, mut b) = (0f32, 0f32, 0f32, 0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32 * 2.0 - 1.0;
                    let v = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32 * 2.0 - 1.0;
                    let (x, y) = (u, -v);
                    let along = (x + y) * std::f32::consts::FRAC_1_SQRT_2;
                    let across = (y - x) * std::f32::consts::FRAC_1_SQRT_2;

                    let in_leaf = along * along + (across - offset).powi(2) <= radius_sq
                        && along * along + (across + offset).powi(2) <= radius_sq;
                    let in_stem = across.abs() < VEIN && along > -1.08 && along < -HALF_LENGTH + 0.05;
                    let color = if in_stem && !in_leaf {
                        stem
                    } else if in_leaf && across.abs() < VEIN * 0.6 && along < HALF_LENGTH * 0.7 {
                        vein
                    } else if in_leaf {
                        leaf
                    } else {
                        continue;
                    };
                    hits += 1.0;
                    r += color.0;
                    g += color.1;
                    b += color.2;
                }
            }
            if hits > 0.0 {
                let alpha = (hits / (SAMPLES * SAMPLES) as f32 * 255.0).round() as u32;
                let (r, g, b) = ((r / hits).round() as u32, (g / hits).round() as u32, (b / hits).round() as u32);
                out[py * size + px] = alpha << 24 | r << 16 | g << 8 | b;
            }
        }
    }
    out
}
