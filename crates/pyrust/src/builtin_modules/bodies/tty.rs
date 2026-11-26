// `tty` module — 100% native Rust implementation of Python 3.12's terminal
// control utilities (`IFLAG`, `OFLAG`, `CFLAG`, `LFLAG`, `ISPEED`, `OSPEED`,
// `CC`, `cfmakeraw`, `cfmakecbreak`, `setraw`, `setcbreak`) (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/tty.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

use super::termios::{tcgetattr_value, tcsetattr_value};

const IFLAG: usize = 0;
const OFLAG: usize = 1;
const CFLAG: usize = 2;
const LFLAG: usize = 3;
const ISPEED: usize = 4;
const OSPEED: usize = 5;
const CC: usize = 6;

fn flag_as_i64(val: &Value) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            "termios mode flags must be integers".to_string(),
        )),
    }
}

fn clone_mode(mode: &Value) -> Result<Value> {
    let cloned = mode.list_with(|items| {
        if items.len() != 7 {
            return Err(PyError::named(
                "TypeError",
                "mode must be a 7-element list".to_string(),
            ));
        }
        let mut out = items.clone();
        let cc_copy = items[CC]
            .list_with(|cc| Value::list(cc.clone()))
            .ok_or_else(|| PyError::named("TypeError", "mode[CC] must be a list".to_string()))?;
        out[CC] = cc_copy;
        Ok(Value::list(out))
    });
    match cloned {
        Some(res) => res,
        None => Err(PyError::named(
            "TypeError",
            "mode must be a list".to_string(),
        )),
    }
}

fn apply_cfmakeraw(mode: &Value) -> Result<()> {
    let res = mode.list_with_mut(|items| {
        if items.len() != 7 {
            return Err(PyError::named(
                "IndexError",
                "mode must be a 7-element list".to_string(),
            ));
        }
        let iflag_mask = (libc::IGNBRK
            | libc::BRKINT
            | libc::IGNPAR
            | libc::PARMRK
            | libc::INPCK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON) as i64;
        let oflag_mask = libc::OPOST as i64;
        let cflag_clear = (libc::CSIZE | libc::PARENB) as i64;
        let cflag_set = libc::CS8 as i64;
        let lflag_mask = (libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG) as i64;

        let new_iflag = flag_as_i64(&items[IFLAG])? & !iflag_mask;
        let new_oflag = flag_as_i64(&items[OFLAG])? & !oflag_mask;
        let new_cflag = (flag_as_i64(&items[CFLAG])? & !cflag_clear) | cflag_set;
        let new_lflag = flag_as_i64(&items[LFLAG])? & !lflag_mask;

        items[IFLAG] = Value::int(new_iflag);
        items[OFLAG] = Value::int(new_oflag);
        items[CFLAG] = Value::int(new_cflag);
        items[LFLAG] = Value::int(new_lflag);

        let cc_res = items[CC].list_with_mut(|cc| {
            if cc.len() <= libc::VMIN || cc.len() <= libc::VTIME {
                return Err(PyError::named(
                    "IndexError",
                    "cc list too short".to_string(),
                ));
            }
            cc[libc::VMIN] = Value::int(1);
            cc[libc::VTIME] = Value::int(0);
            Ok(())
        });
        match cc_res {
            Some(r) => r,
            None => Err(PyError::named(
                "TypeError",
                "mode[CC] must be a list".to_string(),
            )),
        }
    });
    match res {
        Some(r) => r,
        None => Err(PyError::named(
            "TypeError",
            "mode must be a list".to_string(),
        )),
    }
}

fn apply_cfmakecbreak(mode: &Value) -> Result<()> {
    let res = mode.list_with_mut(|items| {
        if items.len() != 7 {
            return Err(PyError::named(
                "IndexError",
                "mode must be a 7-element list".to_string(),
            ));
        }
        let lflag_mask = (libc::ECHO | libc::ICANON) as i64;
        let new_lflag = flag_as_i64(&items[LFLAG])? & !lflag_mask;
        items[LFLAG] = Value::int(new_lflag);

        let cc_res = items[CC].list_with_mut(|cc| {
            if cc.len() <= libc::VMIN || cc.len() <= libc::VTIME {
                return Err(PyError::named(
                    "IndexError",
                    "cc list too short".to_string(),
                ));
            }
            cc[libc::VMIN] = Value::int(1);
            cc[libc::VTIME] = Value::int(0);
            Ok(())
        });
        match cc_res {
            Some(r) => r,
            None => Err(PyError::named(
                "TypeError",
                "mode[CC] must be a list".to_string(),
            )),
        }
    });
    match res {
        Some(r) => r,
        None => Err(PyError::named(
            "TypeError",
            "mode must be a list".to_string(),
        )),
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("cfmakeraw"),
            Value::string("cfmakecbreak"),
            Value::string("setraw"),
            Value::string("setcbreak"),
        ]),
        "IFLAG" => Value::int(IFLAG as i64),
        "OFLAG" => Value::int(OFLAG as i64),
        "CFLAG" => Value::int(CFLAG as i64),
        "LFLAG" => Value::int(LFLAG as i64),
        "ISPEED" => Value::int(ISPEED as i64),
        "OSPEED" => Value::int(OSPEED as i64),
        "CC" => Value::int(CC as i64),
    }

    /// Make termios `mode` raw in place.
    fn cfmakeraw(mode: PyValue) -> Result<Value> {
        apply_cfmakeraw(&mode.0)?;
        Ok(Value::none())
    }

    /// Make termios `mode` cbreak in place.
    fn cfmakecbreak(mode: PyValue) -> Result<Value> {
        apply_cfmakecbreak(&mode.0)?;
        Ok(Value::none())
    }

    /// Put terminal `fd` into raw mode and return its original attributes.
    fn setraw(
        fd: PyValue,
        #[default((libc::TCSAFLUSH as i64).into())] when: PyIndex,
    ) -> Result<Value> {
        let when_c = when.expect_i64(FN_NAME, "when")? as libc::c_int;
        let orig = tcgetattr_value(_interp, &fd.0)?;
        let new_mode = clone_mode(&orig)?;
        apply_cfmakeraw(&new_mode)?;
        tcsetattr_value(_interp, &fd.0, when_c, &new_mode)?;
        Ok(orig)
    }

    /// Put terminal `fd` into cbreak mode and return its original attributes.
    fn setcbreak(
        fd: PyValue,
        #[default((libc::TCSAFLUSH as i64).into())] when: PyIndex,
    ) -> Result<Value> {
        let when_c = when.expect_i64(FN_NAME, "when")? as libc::c_int;
        let orig = tcgetattr_value(_interp, &fd.0)?;
        let new_mode = clone_mode(&orig)?;
        apply_cfmakecbreak(&new_mode)?;
        tcsetattr_value(_interp, &fd.0, when_c, &new_mode)?;
        Ok(orig)
    }
}
