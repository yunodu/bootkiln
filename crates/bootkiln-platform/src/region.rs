//! Regional settings of the signed-in Windows user, for Windows answer files.

use std::io;

use bootkiln_core::unattend::Region;
use windows_sys::Win32::Globalization::{GetSystemDefaultLocaleName, GetUserDefaultLocaleName};
use windows_sys::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH;
use windows_sys::Win32::System::Time::{DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation, TIME_ZONE_ID_INVALID};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutNameW};

/// `KL_NAMELENGTH` (8 hex digits + NUL); defined in `WindowsAndMessaging`, which we
/// would otherwise not need.
const KL_NAMELENGTH: usize = 9;

pub fn host_region() -> io::Result<Region> {
    let locale = |get: unsafe extern "system" fn(*mut u16, i32) -> i32| -> io::Result<String> {
        let mut buf = [0u16; LOCALE_NAME_MAX_LENGTH as usize];
        // SAFETY: buffer length passed matches the buffer.
        let len = unsafe { get(buf.as_mut_ptr(), buf.len() as i32) };
        if len <= 1 {
            return Err(io::Error::last_os_error());
        }
        Ok(String::from_utf16_lossy(&buf[..len as usize - 1]))
    };

    let mut layout = [0u16; KL_NAMELENGTH];
    // SAFETY: KL_NAMELENGTH-sized buffer as the API requires.
    if unsafe { GetKeyboardLayoutNameW(layout.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let layout = String::from_utf16_lossy(&layout[..layout.iter().position(|&c| c == 0).unwrap_or(layout.len())]);
    // SAFETY: thread 0 is the calling thread.
    let language = unsafe { GetKeyboardLayout(0) } as usize & 0xFFFF;

    // SAFETY: zeroed plain-data struct is a valid out-parameter.
    let mut tz: DYNAMIC_TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointer.
    if unsafe { GetDynamicTimeZoneInformation(&mut tz) } == TIME_ZONE_ID_INVALID {
        return Err(io::Error::last_os_error());
    }
    let key = &tz.TimeZoneKeyName;
    let time_zone = String::from_utf16_lossy(&key[..key.iter().position(|&c| c == 0).unwrap_or(key.len())]);

    Ok(Region {
        input_locale: format!("{language:04x}:{layout}"),
        system_locale: locale(GetSystemDefaultLocaleName)?,
        user_locale: locale(GetUserDefaultLocaleName)?,
        time_zone,
    })
}

/// The user's locale as a BCP 47 name such as `de-DE`.
pub fn host_locale() -> Option<String> {
    host_region().ok().map(|r| r.user_locale)
}
