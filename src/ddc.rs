//! DDC/CI access to external monitors through the Windows monitor configuration API (dxva2).

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Devices::Display::{
    DestroyPhysicalMonitor, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, QueryDisplayConfig, SetVCPFeature,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, MC_VCP_CODE_TYPE, PHYSICAL_MONITOR, QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::{BOOL, HANDLE, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

use crate::util::from_wide;

pub const VCP_BRIGHTNESS: u8 = 0x10;
pub const VCP_CONTRAST: u8 = 0x12;
pub const VCP_VOLUME: u8 = 0x62;
pub const VCP_INPUT: u8 = 0x60;
pub const VCP_POWER: u8 = 0xD6;

pub const POWER_ON: u32 = 0x01;
pub const POWER_OFF: u32 = 0x04;

/// Slider features, in display order. Indexes into this array are used across the app.
pub const FEATURES: [(u8, &str); 3] = [
    (VCP_BRIGHTNESS, "Brightness"),
    (VCP_CONTRAST, "Contrast"),
    (VCP_VOLUME, "Volume"),
];

/// Common MCCS input source values. Some monitors use their own codes.
pub const INPUTS: [(&str, u32); 7] = [
    ("HDMI 1", 0x11),
    ("HDMI 2", 0x12),
    ("DisplayPort 1", 0x0F),
    ("DisplayPort 2", 0x10),
    ("USB-C", 0x1B),
    ("DVI", 0x03),
    ("VGA", 0x01),
];

pub struct PhysicalMonitor {
    handle: HANDLE,
    pub name: String,
}

// The handle is only ever used from the worker thread that owns it.
unsafe impl Send for PhysicalMonitor {}

impl Drop for PhysicalMonitor {
    fn drop(&mut self) {
        unsafe {
            DestroyPhysicalMonitor(self.handle);
        }
    }
}

impl PhysicalMonitor {
    /// Returns (current, maximum). DDC/CI is flaky on some monitors, so one retry.
    pub fn get(&self, code: u8) -> Option<(u32, u32)> {
        for _ in 0..2 {
            let mut kind: MC_VCP_CODE_TYPE = 0;
            let (mut cur, mut max) = (0u32, 0u32);
            let ok = unsafe {
                GetVCPFeatureAndVCPFeatureReply(self.handle, code, &mut kind, &mut cur, &mut max)
            };
            if ok != 0 {
                return Some((cur, max));
            }
        }
        None
    }

    pub fn set(&self, code: u8, value: u32) -> bool {
        unsafe { SetVCPFeature(self.handle, code, value) != 0 }
    }
}

pub fn enumerate() -> Vec<PhysicalMonitor> {
    let names = friendly_names();
    let mut hmonitors: Vec<HMONITOR> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect_monitor),
            &mut hmonitors as *mut Vec<HMONITOR> as LPARAM,
        );
    }

    let mut out = Vec::new();
    for hmonitor in hmonitors {
        let device = gdi_device_name(hmonitor);
        let friendly: Vec<&String> = names
            .iter()
            .filter(|(d, _)| *d == device)
            .map(|(_, f)| f)
            .collect();

        let mut count = 0u32;
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) } == 0 || count == 0 {
            continue;
        }
        let mut physical: Vec<PHYSICAL_MONITOR> = vec![unsafe { zeroed() }; count as usize];
        if unsafe { GetPhysicalMonitorsFromHMONITOR(hmonitor, count, physical.as_mut_ptr()) } == 0 {
            continue;
        }
        for (i, pm) in physical.iter().enumerate() {
            let name = match friendly.get(i) {
                Some(f) if !f.is_empty() => (*f).clone(),
                _ => {
                    // PHYSICAL_MONITOR is packed, so copy the field out before borrowing it.
                    let raw = pm.szPhysicalMonitorDescription;
                    let desc = from_wide(&raw);
                    if desc.is_empty() || desc.contains("Generic") {
                        format!("Display {}", out.len() + 1)
                    } else {
                        desc
                    }
                }
            };
            out.push(PhysicalMonitor { handle: pm.hPhysicalMonitor, name });
        }
    }
    out
}

unsafe extern "system" fn collect_monitor(hmonitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data as *mut Vec<HMONITOR>);
    list.push(hmonitor);
    1
}

fn gdi_device_name(hmonitor: HMONITOR) -> String {
    unsafe {
        let mut info: MONITORINFOEXW = zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmonitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
            return String::new();
        }
        from_wide(&info.szDevice)
    }
}

/// (GDI device name like \\.\DISPLAY1, monitor model name like "DELL U2720Q") for each active display.
fn friendly_names() -> Vec<(String, String)> {
    unsafe {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != 0 {
            return Vec::new();
        }
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![zeroed(); path_count as usize];
        let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![zeroed(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            null_mut(),
        ) != 0
        {
            return Vec::new();
        }
        paths.truncate(path_count as usize);

        let mut out = Vec::new();
        for path in &paths {
            let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = zeroed();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }

            let mut target: DISPLAYCONFIG_TARGET_DEVICE_NAME = zeroed();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }

            out.push((
                from_wide(&source.viewGdiDeviceName),
                from_wide(&target.monitorFriendlyDeviceName),
            ));
        }
        out
    }
}
