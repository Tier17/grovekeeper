//! "Start with Windows" through the per-user Run key. No admin rights needed.

use std::ptr::{null, null_mut};

use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

use crate::util::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Grovekeeper";

fn open_run_key() -> Option<HKEY> {
    let path = wide(RUN_KEY);
    let mut key: HKEY = null_mut();
    let status = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, &mut key)
    };
    (status == 0).then_some(key)
}

pub fn is_enabled() -> bool {
    let Some(key) = open_run_key() else { return false };
    let name = wide(VALUE_NAME);
    let mut size = 0u32;
    unsafe {
        let found = RegQueryValueExW(key, name.as_ptr(), null(), null_mut(), null_mut(), &mut size) == 0;
        RegCloseKey(key);
        found
    }
}

pub fn set(enabled: bool) {
    let Some(key) = open_run_key() else { return };
    let name = wide(VALUE_NAME);
    unsafe {
        if enabled {
            let data = wide(&format!("\"{}\"", exe_path()));
            RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32);
        } else {
            RegDeleteValueW(key, name.as_ptr());
        }
        RegCloseKey(key);
    }
}

fn exe_path() -> String {
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..len.min(buf.len())])
}
