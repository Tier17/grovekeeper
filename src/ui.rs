//! Layout and drawing for the popup panel. Plain GDI with an off-screen buffer: no GPU work,
//! and nothing is drawn while the panel is hidden.

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, FillRect, FrameRect, SelectObject, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_END_ELLIPSIS,
    DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, HDC, HFONT,
    OUT_DEFAULT_PRECIS, SRCCOPY, TRANSPARENT,
};

use crate::ddc::FEATURES;
use crate::util::{rgb, wide};
use crate::worker::{percent, Snapshot};

const BG: u32 = rgb(28, 30, 29);
const BORDER: u32 = rgb(58, 62, 60);
const TEXT: u32 = rgb(232, 234, 233);
const MUTED: u32 = rgb(150, 156, 153);
const TRACK: u32 = rgb(64, 68, 66);
const ACCENT: u32 = rgb(108, 186, 92);
const KNOB: u32 = rgb(240, 242, 241);
const KNOB_ACTIVE: u32 = rgb(176, 226, 160);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// Brightness of every monitor at once.
    All,
    /// (monitor index, feature index into `ddc::FEATURES`)
    Mon(usize, usize),
}

pub struct Row {
    pub target: Target,
    pub label: &'static str,
    pub pct: u32,
    pub rect: RECT,
    pub bar: RECT,
    pub value: RECT,
}

pub struct Layout {
    pub width: i32,
    pub height: i32,
    title: RECT,
    headers: Vec<(String, RECT)>,
    pub rows: Vec<Row>,
    message: Option<(&'static str, RECT)>,
    footer: RECT,
    inset: i32,
    track_h: i32,
    knob_w: i32,
    knob_h: i32,
}

pub struct Fonts {
    title: HFONT,
    normal: HFONT,
    small: HFONT,
}

impl Fonts {
    pub fn new(scale: f32) -> Fonts {
        unsafe {
            Fonts {
                title: font(15.0 * scale, 600),
                normal: font(13.0 * scale, 400),
                small: font(12.0 * scale, 400),
            }
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.title);
            DeleteObject(self.normal);
            DeleteObject(self.small);
        }
    }
}

unsafe fn font(px: f32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    CreateFontW(
        -(px.round() as i32),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as _,
        OUT_DEFAULT_PRECIS as _,
        CLIP_DEFAULT_PRECIS as _,
        CLEARTYPE_QUALITY as _,
        (DEFAULT_PITCH as u32 | FF_DONTCARE as u32) as _,
        face.as_ptr(),
    )
}

fn rc(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT { left, top, right, bottom }
}

fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub fn layout(view: &Snapshot, scale: f32) -> Layout {
    let s = |v: f32| (v * scale).round() as i32;
    let width = s(340.0);
    let pad = s(16.0);
    let row_h = s(30.0);
    let label_w = s(84.0);
    let value_w = s(44.0);
    let header_h = s(22.0);

    let mut y = pad;
    let title = rc(pad, y, width - pad, y + s(26.0));
    y += s(34.0);

    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut message = None;

    let row = |target: Target, label: &'static str, pct: u32, y: i32| Row {
        target,
        label,
        pct,
        rect: rc(pad, y, width - pad, y + row_h),
        bar: rc(pad + label_w, y, width - pad - value_w, y + row_h),
        value: rc(width - pad - value_w, y, width - pad, y + row_h),
    };

    if !view.loaded {
        message = Some(("Looking for monitors…", rc(pad, y, width - pad, y + s(24.0))));
        y += s(32.0);
    } else if view.monitors.is_empty() {
        message = Some((
            "No monitors with DDC/CI found. Turn on DDC/CI in your monitor's on-screen menu, \
             then right-click the tray icon and choose Refresh monitors.",
            rc(pad, y, width - pad, y + s(72.0)),
        ));
        y += s(80.0);
    } else {
        let brightness: Vec<u32> = view
            .monitors
            .iter()
            .filter_map(|m| m.features[0].as_ref().map(percent))
            .collect();
        if brightness.len() > 1 {
            let avg = brightness.iter().sum::<u32>() / brightness.len() as u32;
            headers.push(("All monitors".to_string(), rc(pad, y, width - pad, y + header_h)));
            y += header_h;
            rows.push(row(Target::All, "Brightness", avg, y));
            y += row_h + s(12.0);
        }
        for (mi, m) in view.monitors.iter().enumerate() {
            let supported = m.features.iter().any(Option::is_some);
            let name = if supported { m.name.clone() } else { format!("{} (no DDC/CI)", m.name) };
            headers.push((name, rc(pad, y, width - pad, y + header_h)));
            y += header_h;
            for (fi, (_, label)) in FEATURES.iter().enumerate() {
                if let Some(fv) = &m.features[fi] {
                    rows.push(row(Target::Mon(mi, fi), label, percent(fv), y));
                    y += row_h;
                }
            }
            y += s(12.0);
        }
    }

    let footer = rc(pad, y, width - pad, y + s(18.0));
    y += s(18.0) + pad;

    Layout {
        width,
        height: y,
        title,
        headers,
        rows,
        message,
        footer,
        inset: s(6.0),
        track_h: s(4.0).max(2),
        knob_w: s(4.0).max(2),
        knob_h: s(16.0),
    }
}

impl Layout {
    pub fn hit(&self, x: i32, y: i32) -> Option<Target> {
        self.rows.iter().find(|r| contains(&r.rect, x, y)).map(|r| r.target)
    }

    pub fn row(&self, target: Target) -> Option<&Row> {
        self.rows.iter().find(|r| r.target == target)
    }

    pub fn pct_at(&self, row: &Row, x: i32) -> u32 {
        let (left, right) = (row.bar.left + self.inset, row.bar.right - self.inset);
        if right <= left {
            return 0;
        }
        (((x - left) * 100 + (right - left) / 2) / (right - left)).clamp(0, 100) as u32
    }
}

pub unsafe fn paint(hdc: HDC, client: &RECT, lay: &Layout, fonts: &Fonts, active: Option<Target>, status: &str) {
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    let mem = CreateCompatibleDC(hdc);
    let bitmap = CreateCompatibleBitmap(hdc, w, h);
    let old_bitmap = SelectObject(mem, bitmap);

    fill(mem, client, BG);
    let border = CreateSolidBrush(BORDER);
    FrameRect(mem, client, border);
    DeleteObject(border);
    SetBkMode(mem, TRANSPARENT as _);

    let one_line = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;
    text(mem, fonts.title, TEXT, "Grovekeeper", lay.title, DT_LEFT | one_line);
    let status_rect = RECT { left: lay.title.left + (lay.title.right - lay.title.left) / 2, ..lay.title };
    text(mem, fonts.small, ACCENT, status, status_rect, DT_RIGHT | one_line);
    for (name, r) in &lay.headers {
        text(mem, fonts.small, MUTED, name, *r, DT_LEFT | one_line);
    }
    if let Some((msg, r)) = lay.message {
        text(mem, fonts.normal, MUTED, msg, r, DT_LEFT | DT_WORDBREAK);
    }

    for row in &lay.rows {
        text(mem, fonts.normal, TEXT, row.label, row.rect, DT_LEFT | one_line);
        text(mem, fonts.normal, MUTED, &format!("{}%", row.pct), row.value, DT_RIGHT | one_line);

        let (left, right) = (row.bar.left + lay.inset, row.bar.right - lay.inset);
        let mid = (row.bar.top + row.bar.bottom) / 2;
        let top = mid - lay.track_h / 2;
        fill(mem, &rc(left, top, right, top + lay.track_h), TRACK);
        let x = left + (right - left) * row.pct as i32 / 100;
        fill(mem, &rc(left, top, x, top + lay.track_h), ACCENT);
        let knob = if active == Some(row.target) { KNOB_ACTIVE } else { KNOB };
        let (kx, ky) = (x - lay.knob_w / 2, mid - lay.knob_h / 2);
        fill(mem, &rc(kx, ky, kx + lay.knob_w, ky + lay.knob_h), knob);
    }

    text(
        mem,
        fonts.small,
        MUTED,
        "Ctrl+Alt+Up/Down: brightness  ·  Right-click tray: more",
        lay.footer,
        DT_LEFT | one_line,
    );

    BitBlt(hdc, client.left, client.top, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bitmap);
    DeleteObject(bitmap);
    DeleteDC(mem);
}

unsafe fn fill(hdc: HDC, r: &RECT, color: u32) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, r, brush);
    DeleteObject(brush);
}

unsafe fn text(hdc: HDC, font: HFONT, color: u32, s: &str, r: RECT, format: u32) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, color);
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(hdc, buf.as_mut_ptr() as _, buf.len() as i32, &mut r, format as _);
    SelectObject(hdc, old);
}
