//! Minimal logcat writer. Messages must never contain secrets, host names or
//! user content; callers pass fixed diagnostic text and stable codes only.

use std::ffi::CString;

const ANDROID_LOG_INFO: i32 = 4;
const ANDROID_LOG_WARN: i32 = 5;

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(
        priority: i32,
        tag: *const std::ffi::c_char,
        text: *const std::ffi::c_char,
    ) -> i32;
}

fn write(priority: i32, message: &str) {
    let Ok(tag) = CString::new("NoriShellBridge") else {
        return;
    };
    let Ok(text) = CString::new(message.replace('\0', " ")) else {
        return;
    };
    // SAFETY: both pointers are valid NUL-terminated strings for the call duration.
    unsafe {
        __android_log_write(priority, tag.as_ptr(), text.as_ptr());
    }
}

pub fn info(message: &str) {
    write(ANDROID_LOG_INFO, message);
}

pub fn warn(message: &str) {
    write(ANDROID_LOG_WARN, message);
}
