// `termios` module — 100% native Rust implementation of Python 3.12's POSIX
// terminal control interface (`tcgetattr`, `tcsetattr`, `tcsendbreak`,
// `tcdrain`, `tcflush`, `tcflow`, `tcgetwinsize`, `tcsetwinsize`, `error`,
// and standard constants) (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/termios.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

use super::fcntl::extract_c_int_fd;

fn strip_os_error_suffix(raw: &str) -> String {
    match raw.rfind(" (os error ") {
        Some(idx) => raw[..idx].to_string(),
        None => raw.to_string(),
    }
}

pub(crate) fn raise_termios_error(interp: &mut Interpreter, err: &std::io::Error) -> PyError {
    if let Ok(t_mod) = interp.load_module("termios")
        && let Ok(err_cls) = interp.get_attr(&t_mod, "error")
    {
        let code = err.raw_os_error().unwrap_or(0) as i64;
        let msg = strip_os_error_suffix(&err.to_string());
        let args = [
            ExpandedCallArg {
                name: None,
                value: Value::int(code),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(msg),
            },
        ];
        if let Ok(inst) = interp.call_function_expanded(err_cls, &args) {
            return PyError::raised(inst);
        }
    }
    PyError::from_io_error(err, None)
}

fn value_as_u64_flag(val: &Value, name: &str) -> Result<u64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n as u64),
        ValueKind::Bool(b) => Ok(u64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "tcsetattr(): {name} must be an integer, not {}",
                value_type_name_str(val)
            ),
        )),
    }
}

fn value_as_cc_byte(val: &Value) -> Result<libc::cc_t> {
    match val.kind() {
        ValueKind::Int(n) => {
            if !(0..=255).contains(&n) {
                return Err(PyError::named(
                    "OverflowError",
                    "termios cc integer out of range".to_string(),
                ));
            }
            Ok(n as libc::cc_t)
        }
        ValueKind::Bool(b) => Ok(libc::cc_t::from(b)),
        ValueKind::Bytes(b) => {
            if b.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    "tcsetattr: elements of attributes must be characters or integers".to_string(),
                ));
            }
            Ok(b[0])
        }
        _ => Err(PyError::named(
            "TypeError",
            "tcsetattr: elements of attributes must be characters or integers".to_string(),
        )),
    }
}

/// Shared helper used by both `termios.tcgetattr` and `tty.setraw`/`setcbreak`.
pub(crate) fn tcgetattr_value(interp: &mut Interpreter, fd: &Value) -> Result<Value> {
    let raw_fd = extract_c_int_fd(interp, fd)?;
    // SAFETY: zero-initialized `libc::termios` passed as out-pointer to `tcgetattr`.
    let mut mode: libc::termios = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::tcgetattr(raw_fd, &mut mode) };
    if rc < 0 {
        return Err(raise_termios_error(
            interp,
            &std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: `mode` was populated by `tcgetattr`.
    let ispeed = unsafe { libc::cfgetispeed(&mode) };
    let ospeed = unsafe { libc::cfgetospeed(&mode) };

    let icanon_off = (mode.c_lflag & libc::ICANON) == 0;
    let mut cc = Vec::with_capacity(libc::NCCS);
    for (idx, &byte) in mode.c_cc.iter().enumerate() {
        if icanon_off && (idx == libc::VMIN || idx == libc::VTIME) {
            cc.push(Value::int(byte as i64));
        } else {
            cc.push(Value::bytes(vec![byte]));
        }
    }

    Ok(Value::list(vec![
        Value::int(mode.c_iflag as i64),
        Value::int(mode.c_oflag as i64),
        Value::int(mode.c_cflag as i64),
        Value::int(mode.c_lflag as i64),
        Value::int(ispeed as i64),
        Value::int(ospeed as i64),
        Value::list(cc),
    ]))
}

/// Shared helper used by both `termios.tcsetattr` and `tty.setraw`/`setcbreak`.
pub(crate) fn tcsetattr_value(
    interp: &mut Interpreter,
    fd: &Value,
    when: libc::c_int,
    attributes: &Value,
) -> Result<()> {
    let raw_fd = extract_c_int_fd(interp, fd)?;
    let items = interp.collect_iterable(attributes).map_err(|_| {
        PyError::named(
            "TypeError",
            "tcsetattr, arg 3: must be 7 element list".to_string(),
        )
    })?;
    if items.len() != 7 {
        return Err(PyError::named(
            "TypeError",
            "tcsetattr, arg 3: must be 7 element list".to_string(),
        ));
    }

    let iflag = value_as_u64_flag(&items[0], "iflag")? as libc::tcflag_t;
    let oflag = value_as_u64_flag(&items[1], "oflag")? as libc::tcflag_t;
    let cflag = value_as_u64_flag(&items[2], "cflag")? as libc::tcflag_t;
    let lflag = value_as_u64_flag(&items[3], "lflag")? as libc::tcflag_t;
    let ispeed = value_as_u64_flag(&items[4], "ispeed")? as libc::speed_t;
    let ospeed = value_as_u64_flag(&items[5], "ospeed")? as libc::speed_t;

    let cc_items = interp.collect_iterable(&items[6]).map_err(|_| {
        PyError::named(
            "TypeError",
            "tcsetattr, arg 3: cc must be a sequence".to_string(),
        )
    })?;
    if cc_items.len() != libc::NCCS {
        return Err(PyError::named(
            "TypeError",
            format!("tcsetattr: cc must have {} elements", libc::NCCS),
        ));
    }

    // Start from existing termios to preserve any platform-private fields.
    // SAFETY: zero-initialized `libc::termios`.
    let mut mode: libc::termios = unsafe { std::mem::zeroed() };
    let _ = unsafe { libc::tcgetattr(raw_fd, &mut mode) };
    mode.c_iflag = iflag;
    mode.c_oflag = oflag;
    mode.c_cflag = cflag;
    mode.c_lflag = lflag;
    for (slot, item) in mode.c_cc.iter_mut().zip(cc_items.iter()) {
        *slot = value_as_cc_byte(item)?;
    }

    // SAFETY: `mode` is a valid initialized `libc::termios`.
    if unsafe { libc::cfsetispeed(&mut mode, ispeed) } < 0 {
        return Err(raise_termios_error(
            interp,
            &std::io::Error::last_os_error(),
        ));
    }
    if unsafe { libc::cfsetospeed(&mut mode, ospeed) } < 0 {
        return Err(raise_termios_error(
            interp,
            &std::io::Error::last_os_error(),
        ));
    }
    if unsafe { libc::tcsetattr(raw_fd, when, &mode) } < 0 {
        return Err(raise_termios_error(
            interp,
            &std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

pyrust_module! {
    constants {
        // Optional actions for tcsetattr
        "TCSANOW" => Value::int(libc::TCSANOW as i64),
        "TCSADRAIN" => Value::int(libc::TCSADRAIN as i64),
        "TCSAFLUSH" => Value::int(libc::TCSAFLUSH as i64),

        // Queue selectors for tcflush
        "TCIFLUSH" => Value::int(libc::TCIFLUSH as i64),
        "TCOFLUSH" => Value::int(libc::TCOFLUSH as i64),
        "TCIOFLUSH" => Value::int(libc::TCIOFLUSH as i64),

        // Actions for tcflow
        "TCOOFF" => Value::int(libc::TCOOFF as i64),
        "TCOON" => Value::int(libc::TCOON as i64),
        "TCIOFF" => Value::int(libc::TCIOFF as i64),
        "TCION" => Value::int(libc::TCION as i64),

        // Special control characters
        "NCCS" => Value::int(libc::NCCS as i64),
        "VINTR" => Value::int(libc::VINTR as i64),
        "VQUIT" => Value::int(libc::VQUIT as i64),
        "VERASE" => Value::int(libc::VERASE as i64),
        "VKILL" => Value::int(libc::VKILL as i64),
        "VEOF" => Value::int(libc::VEOF as i64),
        "VTIME" => Value::int(libc::VTIME as i64),
        "VMIN" => Value::int(libc::VMIN as i64),
        "VSTART" => Value::int(libc::VSTART as i64),
        "VSTOP" => Value::int(libc::VSTOP as i64),
        "VSUSP" => Value::int(libc::VSUSP as i64),
        "VEOL" => Value::int(libc::VEOL as i64),
        "VREPRINT" => Value::int(libc::VREPRINT as i64),
        "VDISCARD" => Value::int(libc::VDISCARD as i64),
        "VWERASE" => Value::int(libc::VWERASE as i64),
        "VLNEXT" => Value::int(libc::VLNEXT as i64),
        "VEOL2" => Value::int(libc::VEOL2 as i64),

        // Input modes (c_iflag)
        "IGNBRK" => Value::int(libc::IGNBRK as i64),
        "BRKINT" => Value::int(libc::BRKINT as i64),
        "IGNPAR" => Value::int(libc::IGNPAR as i64),
        "PARMRK" => Value::int(libc::PARMRK as i64),
        "INPCK" => Value::int(libc::INPCK as i64),
        "ISTRIP" => Value::int(libc::ISTRIP as i64),
        "INLCR" => Value::int(libc::INLCR as i64),
        "IGNCR" => Value::int(libc::IGNCR as i64),
        "ICRNL" => Value::int(libc::ICRNL as i64),
        "IXON" => Value::int(libc::IXON as i64),
        "IXOFF" => Value::int(libc::IXOFF as i64),
        "IXANY" => Value::int(libc::IXANY as i64),
        "IMAXBEL" => Value::int(libc::IMAXBEL as i64),

        // Output modes (c_oflag)
        "OPOST" => Value::int(libc::OPOST as i64),
        "ONLCR" => Value::int(libc::ONLCR as i64),
        "OCRNL" => Value::int(libc::OCRNL as i64),
        "ONOCR" => Value::int(libc::ONOCR as i64),
        "ONLRET" => Value::int(libc::ONLRET as i64),

        // Control modes (c_cflag)
        "CSIZE" => Value::int(libc::CSIZE as i64),
        "CS5" => Value::int(libc::CS5 as i64),
        "CS6" => Value::int(libc::CS6 as i64),
        "CS7" => Value::int(libc::CS7 as i64),
        "CS8" => Value::int(libc::CS8 as i64),
        "CSTOPB" => Value::int(libc::CSTOPB as i64),
        "CREAD" => Value::int(libc::CREAD as i64),
        "PARENB" => Value::int(libc::PARENB as i64),
        "PARODD" => Value::int(libc::PARODD as i64),
        "HUPCL" => Value::int(libc::HUPCL as i64),
        "CLOCAL" => Value::int(libc::CLOCAL as i64),

        // Local modes (c_lflag)
        "ISIG" => Value::int(libc::ISIG as i64),
        "ICANON" => Value::int(libc::ICANON as i64),
        "ECHO" => Value::int(libc::ECHO as i64),
        "ECHOE" => Value::int(libc::ECHOE as i64),
        "ECHOK" => Value::int(libc::ECHOK as i64),
        "ECHONL" => Value::int(libc::ECHONL as i64),
        "NOFLSH" => Value::int(libc::NOFLSH as i64),
        "TOSTOP" => Value::int(libc::TOSTOP as i64),
        "IEXTEN" => Value::int(libc::IEXTEN as i64),

        // Baud rates
        "B0" => Value::int(libc::B0 as i64),
        "B50" => Value::int(libc::B50 as i64),
        "B75" => Value::int(libc::B75 as i64),
        "B110" => Value::int(libc::B110 as i64),
        "B134" => Value::int(libc::B134 as i64),
        "B150" => Value::int(libc::B150 as i64),
        "B200" => Value::int(libc::B200 as i64),
        "B300" => Value::int(libc::B300 as i64),
        "B600" => Value::int(libc::B600 as i64),
        "B1200" => Value::int(libc::B1200 as i64),
        "B1800" => Value::int(libc::B1800 as i64),
        "B2400" => Value::int(libc::B2400 as i64),
        "B4800" => Value::int(libc::B4800 as i64),
        "B9600" => Value::int(libc::B9600 as i64),
        "B19200" => Value::int(libc::B19200 as i64),
        "B38400" => Value::int(libc::B38400 as i64),
        "B57600" => Value::int(libc::B57600 as i64),
        "B115200" => Value::int(libc::B115200 as i64),
        "B230400" => Value::int(libc::B230400 as i64),

        // Window-size ioctl request constants
        "TIOCGWINSZ" => Value::int(libc::TIOCGWINSZ as i64),
        "TIOCSWINSZ" => Value::int(libc::TIOCSWINSZ as i64),
    }

    /// Exception raised when a termios operation fails.
    exception error;

    /// Get the tty attributes for file descriptor `fd`.
    fn tcgetattr(#[positional_only] fd: PyValue) -> Result<Value> {
        tcgetattr_value(_interp, &fd.0)
    }

    /// Set the tty attributes for file descriptor `fd`.
    fn tcsetattr(
        #[positional_only] fd: PyValue,
        #[positional_only] when: PyIndex,
        #[positional_only] attributes: PyValue,
    ) -> Result<Value> {
        let when_c = when.expect_i64(FN_NAME, "when")? as libc::c_int;
        tcsetattr_value(_interp, &fd.0, when_c, &attributes.0)?;
        Ok(Value::none())
    }

    /// Send a break on file descriptor `fd`.
    fn tcsendbreak(
        #[positional_only] fd: PyValue,
        #[positional_only] duration: PyIndex,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let dur = duration.expect_i64(FN_NAME, "duration")? as libc::c_int;
        // SAFETY: standard POSIX `tcsendbreak`.
        if unsafe { libc::tcsendbreak(raw_fd, dur) } < 0 {
            return Err(raise_termios_error(
                _interp,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(Value::none())
    }

    /// Wait until all output written to file descriptor `fd` has been transmitted.
    fn tcdrain(#[positional_only] fd: PyValue) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        // SAFETY: standard POSIX `tcdrain`.
        if unsafe { libc::tcdrain(raw_fd) } < 0 {
            return Err(raise_termios_error(
                _interp,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(Value::none())
    }

    /// Discard queued data on file descriptor `fd`.
    fn tcflush(
        #[positional_only] fd: PyValue,
        #[positional_only] queue: PyIndex,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let q = queue.expect_i64(FN_NAME, "queue")? as libc::c_int;
        // SAFETY: standard POSIX `tcflush`.
        if unsafe { libc::tcflush(raw_fd, q) } < 0 {
            return Err(raise_termios_error(
                _interp,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(Value::none())
    }

    /// Suspend or resume input or output on file descriptor `fd`.
    fn tcflow(
        #[positional_only] fd: PyValue,
        #[positional_only] action: PyIndex,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let act = action.expect_i64(FN_NAME, "action")? as libc::c_int;
        // SAFETY: standard POSIX `tcflow`.
        if unsafe { libc::tcflow(raw_fd, act) } < 0 {
            return Err(raise_termios_error(
                _interp,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(Value::none())
    }

    /// Get the tty window size for file descriptor `fd` as `(rows, cols)`.
    fn tcgetwinsize(#[positional_only] fd: PyValue) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        // SAFETY: zero-initialized `libc::winsize` out-pointer for `TIOCGWINSZ`.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::ioctl(raw_fd, libc::TIOCGWINSZ, &mut ws) };
        if rc < 0 {
            return Err(raise_termios_error(
                _interp,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(Value::tuple(vec![
            Value::int(ws.ws_row as i64),
            Value::int(ws.ws_col as i64),
        ]))
    }

    /// Set the tty window size for file descriptor `fd` from `(rows, cols)`.
    fn tcsetwinsize(
        #[positional_only] fd: PyValue,
        #[positional_only] winsize: PyValue,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let items = _interp.collect_iterable(&winsize.0).map_err(|_| {
            PyError::named(
                "TypeError",
                "tcsetwinsize, arg 2: must be a two-item sequence".to_string(),
            )
        })?;
        if items.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                "tcsetwinsize, arg 2: must be a two-item sequence".to_string(),
            ));
        }
        let rows = match items[0].kind() {
            ValueKind::Int(n) if (0..=u16::MAX as i64).contains(&n) => n as libc::c_ushort,
            ValueKind::Int(_) => {
                return Err(PyError::named(
                    "OverflowError",
                    "unsigned short integer is out of range".to_string(),
                ));
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "tcsetwinsize(): rows must be an integer".to_string(),
                ));
            }
        };
        let cols = match items[1].kind() {
            ValueKind::Int(n) if (0..=u16::MAX as i64).contains(&n) => n as libc::c_ushort,
            ValueKind::Int(_) => {
                return Err(PyError::named(
                    "OverflowError",
                    "unsigned short integer is out of range".to_string(),
                ));
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "tcsetwinsize(): cols must be an integer".to_string(),
                ));
            }
        };
        // Preserve existing pixel dimensions if TIOCGWINSZ succeeds.
        // SAFETY: zero-initialized `libc::winsize`.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let _ = unsafe { libc::ioctl(raw_fd, libc::TIOCGWINSZ, &mut ws) };
        ws.ws_row = rows;
        ws.ws_col = cols;
        // SAFETY: `ws` is a valid `libc::winsize` pointer.
        let rc = unsafe { libc::ioctl(raw_fd, libc::TIOCSWINSZ, &ws) };
        if rc < 0 {
            return Err(raise_termios_error(
                _interp,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(Value::none())
    }
}
