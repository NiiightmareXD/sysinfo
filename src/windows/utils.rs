// Take a look at the license at the top of the repository in the LICENSE file.

use std::{ffi::OsStr, os::windows::ffi::OsStrExt, time::SystemTime};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, WIN32_ERROR},
        System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_READ, REG_VALUE_TYPE,
        },
    },
};

#[inline]
pub(crate) fn filetime_to_u64(f: FILETIME) -> u64 {
    (f.dwHighDateTime as u64) << 32 | (f.dwLowDateTime as u64)
}

#[inline]
pub(crate) fn get_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|n| n.as_secs())
        .unwrap_or(0)
}

fn utf16_str<S: AsRef<OsStr> + ?Sized>(text: &S) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>()
}

struct RegKey(HKEY);

impl RegKey {
    unsafe fn open(hkey: HKEY, path: &[u16]) -> Option<Self> {
        let mut new_hkey = HKEY::default();
        if RegOpenKeyExW(hkey, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut new_hkey).is_err() {
            return None;
        }
        Some(Self(new_hkey))
    }

    unsafe fn get_value(&self, field_name: &[u16], buf: &mut [u8], buf_len: &mut u32) -> u32 {
        let mut buf_type = REG_VALUE_TYPE::default();

        let res = RegQueryValueExW(
            self.0,
            PCWSTR(field_name.as_ptr()),
            None,
            Some(&mut buf_type),
            Some(buf.as_mut_ptr()),
            Some(buf_len),
        );

        if res.is_ok() {
            0
        } else {
            res.err().unwrap_unchecked().code().0 as u32
        }
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

pub(crate) fn get_reg_string_value(hkey: HKEY, path: &str, field_name: &str) -> Option<String> {
    let c_path = utf16_str(path);
    let c_field_name = utf16_str(field_name);

    unsafe {
        let new_key = RegKey::open(hkey, &c_path)?;
        let mut buf_len = 2048;
        let mut buf: Vec<u8> = Vec::with_capacity(buf_len as usize);

        loop {
            match WIN32_ERROR(new_key.get_value(&c_field_name, &mut buf, &mut buf_len)) {
                ERROR_SUCCESS => break,
                ERROR_MORE_DATA => {
                    buf.reserve(buf_len as _);
                }
                _ => return None,
            }
        }

        buf.set_len(buf_len as _);

        let words = std::slice::from_raw_parts(buf.as_ptr() as *const u16, buf.len() / 2);
        let mut s = String::from_utf16_lossy(words);
        while s.ends_with('\u{0}') {
            s.pop();
        }
        Some(s)
    }
}

pub(crate) fn get_reg_value_u32(hkey: HKEY, path: &str, field_name: &str) -> Option<[u8; 4]> {
    let c_path = utf16_str(path);
    let c_field_name = utf16_str(field_name);

    unsafe {
        let new_key = RegKey::open(hkey, &c_path)?;
        let mut buf_len: u32 = 4;
        let mut buf = [0u8; 4];

        match new_key.get_value(&c_field_name, &mut buf, &mut buf_len) {
            0 => Some(buf),
            _ => None,
        }
    }
}
