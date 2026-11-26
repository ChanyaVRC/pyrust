// Native Rust `winsound` module (Windows-only, issue #3262).
//
// Reference: <https://docs.python.org/3.12/library/winsound.html>

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use pyrust_derive::pyrust_module;
use windows_sys::Win32::Media::Audio::{
    PlaySoundW, SND_ALIAS, SND_APPLICATION, SND_ASYNC, SND_FILENAME, SND_LOOP, SND_MEMORY,
    SND_NODEFAULT, SND_NOSTOP, SND_NOWAIT, SND_PURGE, SND_SENTRY, SND_SYNC, SND_SYSTEM,
};
use windows_sys::Win32::System::Diagnostics::Debug::{Beep, MessageBeep};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONASTERISK, MB_ICONERROR, MB_ICONEXCLAMATION, MB_ICONHAND, MB_ICONINFORMATION,
    MB_ICONQUESTION, MB_ICONSTOP, MB_ICONWARNING, MB_OK,
};

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBytesLike, PyIndex, PyValueRef};
use crate::value::Value;

pyrust_module! {
    constants {
        "SND_SYNC" => Value::int(i64::from(SND_SYNC)),
        "SND_ASYNC" => Value::int(i64::from(SND_ASYNC)),
        "SND_NODEFAULT" => Value::int(i64::from(SND_NODEFAULT)),
        "SND_MEMORY" => Value::int(i64::from(SND_MEMORY)),
        "SND_LOOP" => Value::int(i64::from(SND_LOOP)),
        "SND_NOSTOP" => Value::int(i64::from(SND_NOSTOP)),
        "SND_PURGE" => Value::int(i64::from(SND_PURGE)),
        "SND_APPLICATION" => Value::int(i64::from(SND_APPLICATION)),
        "SND_NOWAIT" => Value::int(i64::from(SND_NOWAIT)),
        "SND_ALIAS" => Value::int(i64::from(SND_ALIAS)),
        "SND_FILENAME" => Value::int(i64::from(SND_FILENAME)),
        "SND_SENTRY" => Value::int(i64::from(SND_SENTRY)),
        "SND_SYSTEM" => Value::int(i64::from(SND_SYSTEM)),

        "MB_OK" => Value::int(i64::from(MB_OK)),
        "MB_ICONHAND" => Value::int(i64::from(MB_ICONHAND)),
        "MB_ICONSTOP" => Value::int(i64::from(MB_ICONSTOP)),
        "MB_ICONERROR" => Value::int(i64::from(MB_ICONERROR)),
        "MB_ICONQUESTION" => Value::int(i64::from(MB_ICONQUESTION)),
        "MB_ICONEXCLAMATION" => Value::int(i64::from(MB_ICONEXCLAMATION)),
        "MB_ICONWARNING" => Value::int(i64::from(MB_ICONWARNING)),
        "MB_ICONASTERISK" => Value::int(i64::from(MB_ICONASTERISK)),
        "MB_ICONINFORMATION" => Value::int(i64::from(MB_ICONINFORMATION)),
    }

    #[py_name = "Beep"]
    fn beep(frequency: PyIndex, duration: PyIndex) -> Result<Value> {
        let freq = frequency.expect_i64(FN_NAME, "frequency")?;
        let dur = duration.expect_i64(FN_NAME, "duration")?;
        if !(37..=32767).contains(&freq) {
            return Err(PyError::named(
                "ValueError",
                "frequency must be in 37 thru 32767".to_string(),
            ));
        }
        // SAFETY: `Beep` takes validated frequency and duration integers.
        let ok = unsafe { Beep(freq as u32, dur as u32) };
        if ok == 0 {
            return Err(PyError::named(
                "RuntimeError",
                "Failed to beep".to_string(),
            ));
        }
        Ok(Value::none())
    }

    #[py_name = "MessageBeep"]
    fn message_beep(#[default(0_i64.into())] r#type: PyIndex) -> Result<Value> {
        let kind = r#type.expect_i64(FN_NAME, "type")?;
        // SAFETY: `MessageBeep` accepts any Win32 `MB_*` style value (or -1).
        let ok = unsafe { MessageBeep(kind as u32) };
        if ok == 0 {
            return Err(PyError::named(
                "RuntimeError",
                "Failed to beep".to_string(),
            ));
        }
        Ok(Value::none())
    }

    #[py_name = "PlaySound"]
    fn play_sound(sound: Option<PyValueRef<'_>>, flags: PyIndex) -> Result<Value> {
        let fl = flags.expect_i64(FN_NAME, "flags")? as u32;
        if (fl & SND_ASYNC) != 0 && (fl & SND_MEMORY) != 0 {
            return Err(PyError::named(
                "RuntimeError",
                "Cannot play asynchronously from memory".to_string(),
            ));
        }

        let ok = match sound {
            None => {
                // SAFETY: null sound pointer is valid (e.g. `SND_PURGE`).
                unsafe { PlaySoundW(std::ptr::null(), std::ptr::null_mut(), fl) }
            }
            Some(val) if (fl & SND_MEMORY) != 0 => {
                let buf = PyBytesLike::try_from_value_with_interp(_interp, val.0, FN_NAME, "sound")?;
                let bytes = buf.as_bytes();
                let ptr = if bytes.is_empty() {
                    std::ptr::null()
                } else {
                    bytes.as_ptr().cast::<u16>()
                };
                // SAFETY: synchronous `SND_MEMORY` call borrows `bytes` for the
                // duration of `PlaySoundW` (`SND_ASYNC` was rejected above).
                unsafe { PlaySoundW(ptr, std::ptr::null_mut(), fl) }
            }
            Some(val) => {
                let Some(s) = val.0.as_str() else {
                    return Err(PyError::named(
                        "TypeError",
                        "sound must be str or None".to_string(),
                    ));
                };
                let mut wide: Vec<u16> = OsStr::new(s).encode_wide().collect();
                if wide.contains(&0) {
                    return Err(PyError::named(
                        "ValueError",
                        "embedded null character".to_string(),
                    ));
                }
                wide.push(0);
                // SAFETY: `wide` is NUL-terminated UTF-16 valid for the call.
                unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), fl) }
            }
        };

        if ok == 0 {
            return Err(PyError::named(
                "RuntimeError",
                "Failed to play sound".to_string(),
            ));
        }
        Ok(Value::none())
    }
}
