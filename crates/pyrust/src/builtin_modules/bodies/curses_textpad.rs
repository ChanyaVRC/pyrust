// `curses.textpad` module — 100% native Rust implementation of Python 3.12's
// `Lib/curses/textpad.py` (`rectangle`, `Textbox`) (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.
// Reference: <https://docs.python.org/3.12/library/curses.html#module-curses.textpad>

use pyrust_derive::pyrust_module;

use crate::builtin_modules::curses_engine as eng;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyCallable, PyIndex, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let f = interp.get_attr(obj, name)?;
    call_pos(interp, f, args)
}

fn unpack_pair_i64(interp: &mut Interpreter, val: &Value) -> Result<(i64, i64)> {
    let items = interp.collect_iterable(val)?;
    if items.len() != 2 {
        return Err(PyError::named(
            "ValueError",
            "expected a 2-tuple of integers".to_string(),
        ));
    }
    let a = match items[0].kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "expected integer coordinate".to_string(),
            ));
        }
    };
    let b = match items[1].kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "expected integer coordinate".to_string(),
            ));
        }
    };
    Ok((a, b))
}

fn val_as_ch_i64(val: &Value) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        ValueKind::Str(s) => {
            let mut cs = s.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => Ok(c as i64),
                _ => Err(PyError::named(
                    "TypeError",
                    "expected single character or int".to_string(),
                )),
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            "expected single character or int".to_string(),
        )),
    }
}

fn textbox_update_max_yx(interp: &mut Interpreter, self_val: &Value) -> Result<(i64, i64)> {
    let win = interp.get_attr(self_val, "win")?;
    let maxyx = call_method(interp, &win, "getmaxyx", &[])?;
    let (maxy_raw, maxx_raw) = unpack_pair_i64(interp, &maxyx)?;
    let maxy = maxy_raw - 1;
    let maxx = maxx_raw - 1;
    interp.assign_attr(self_val.clone(), "maxy", Value::int(maxy))?;
    interp.assign_attr(self_val.clone(), "maxx", Value::int(maxx))?;
    Ok((maxy, maxx))
}

fn textbox_end_of_line(interp: &mut Interpreter, self_val: &Value, y: i64) -> Result<i64> {
    let (_, maxx) = textbox_update_max_yx(interp, self_val)?;
    let win = interp.get_attr(self_val, "win")?;
    let mut last = maxx;
    loop {
        let ch_val = call_method(interp, &win, "inch", &[Value::int(y), Value::int(last)])?;
        let ch = val_as_ch_i64(&ch_val)? & 0x7f;
        if ch != 0x20 {
            last = maxx.min(last + 1);
            break;
        } else if last == 0 {
            break;
        }
        last -= 1;
    }
    Ok(last)
}

fn textbox_insert_printable_char(
    interp: &mut Interpreter,
    self_val: &Value,
    mut ch: i64,
) -> Result<()> {
    let (maxy, maxx) = textbox_update_max_yx(interp, self_val)?;
    let win = interp.get_attr(self_val, "win")?;
    let insert_mode_val = interp.get_attr(self_val, "insert_mode")?;
    let insert_mode = interp.truthy_value(&insert_mode_val)?;
    let yx = call_method(interp, &win, "getyx", &[])?;
    let (mut y, mut x) = unpack_pair_i64(interp, &yx)?;
    let mut backyx: Option<(i64, i64)> = None;

    while y < maxy || x < maxx {
        let mut oldch: i64 = 0;
        if insert_mode {
            let ov = call_method(interp, &win, "inch", &[])?;
            oldch = val_as_ch_i64(&ov)?;
        }
        // Ignore curses.error when writing to bottom-right spot (matches CPython).
        let _ = call_method(interp, &win, "addch", &[Value::int(ch)]);
        if !insert_mode || !(32..=126).contains(&oldch) {
            break;
        }
        ch = oldch;
        let yx2 = call_method(interp, &win, "getyx", &[])?;
        let (ny, nx) = unpack_pair_i64(interp, &yx2)?;
        y = ny;
        x = nx;
        if backyx.is_none() {
            backyx = Some((y, x));
        }
    }
    if let Some((by, bx)) = backyx {
        call_method(interp, &win, "move", &[Value::int(by), Value::int(bx)])?;
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("curses"),
    }

    /// Draw a rectangle with corners at the provided upper-left and lower-right coordinates.
    fn rectangle(
        win: PyValue,
        uly: PyIndex,
        ulx: PyIndex,
        lry: PyIndex,
        lrx: PyIndex,
    ) -> Result<Value> {
        let uly_i = uly.expect_i64(FN_NAME, "uly")?;
        let ulx_i = ulx.expect_i64(FN_NAME, "ulx")?;
        let lry_i = lry.expect_i64(FN_NAME, "lry")?;
        let lrx_i = lrx.expect_i64(FN_NAME, "lrx")?;

        let curses_mod = _interp.load_module("curses")?;
        let acs_vline = _interp.get_attr(&curses_mod, "ACS_VLINE")?;
        let acs_hline = _interp.get_attr(&curses_mod, "ACS_HLINE")?;
        let acs_ul = _interp.get_attr(&curses_mod, "ACS_ULCORNER")?;
        let acs_ur = _interp.get_attr(&curses_mod, "ACS_URCORNER")?;
        let acs_lr = _interp.get_attr(&curses_mod, "ACS_LRCORNER")?;
        let acs_ll = _interp.get_attr(&curses_mod, "ACS_LLCORNER")?;

        call_method(
            _interp,
            &win.0,
            "vline",
            &[
                Value::int(uly_i + 1),
                Value::int(ulx_i),
                acs_vline.clone(),
                Value::int(lry_i - uly_i - 1),
            ],
        )?;
        call_method(
            _interp,
            &win.0,
            "hline",
            &[
                Value::int(uly_i),
                Value::int(ulx_i + 1),
                acs_hline.clone(),
                Value::int(lrx_i - ulx_i - 1),
            ],
        )?;
        call_method(
            _interp,
            &win.0,
            "hline",
            &[
                Value::int(lry_i),
                Value::int(ulx_i + 1),
                acs_hline,
                Value::int(lrx_i - ulx_i - 1),
            ],
        )?;
        call_method(
            _interp,
            &win.0,
            "vline",
            &[
                Value::int(uly_i + 1),
                Value::int(lrx_i),
                acs_vline,
                Value::int(lry_i - uly_i - 1),
            ],
        )?;
        call_method(
            _interp,
            &win.0,
            "addch",
            &[Value::int(uly_i), Value::int(ulx_i), acs_ul],
        )?;
        call_method(
            _interp,
            &win.0,
            "addch",
            &[Value::int(uly_i), Value::int(lrx_i), acs_ur],
        )?;
        call_method(
            _interp,
            &win.0,
            "addch",
            &[Value::int(lry_i), Value::int(lrx_i), acs_lr],
        )?;
        call_method(
            _interp,
            &win.0,
            "addch",
            &[Value::int(lry_i), Value::int(ulx_i), acs_ll],
        )?;
        Ok(Value::none())
    }

    /// Editing widget using the interior of a curses window object.
    class Textbox {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            win: PyValue,
            #[default(OptionalArg::Omitted)] insert_mode: OptValueRef<'_>,
        ) -> Result<Value> {
            let ins = match insert_mode {
                OptionalArg::Present(v) => v.0.clone(),
                OptionalArg::Omitted => Value::bool(false),
            };
            _interp.assign_attr(self_val.0.clone(), "win", win.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "insert_mode", ins)?;
            textbox_update_max_yx(_interp, self_val.0)?;
            _interp.assign_attr(self_val.0.clone(), "stripspaces", Value::int(1))?;
            _interp.assign_attr(self_val.0.clone(), "lastcmd", Value::none())?;
            call_method(_interp, &win.0, "keypad", &[Value::int(1)])?;
            Ok(Value::none())
        }

        fn _update_max_yx(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            textbox_update_max_yx(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn _end_of_line(#[positional_only] self_val: PyValueRef<'_>, y: PyIndex) -> Result<Value> {
            let yi = y.expect_i64(FN_NAME, "y")?;
            let last = textbox_end_of_line(_interp, self_val.0, yi)?;
            Ok(Value::int(last))
        }

        fn _insert_printable_char(
            #[positional_only] self_val: PyValueRef<'_>,
            ch: PyValue,
        ) -> Result<Value> {
            let code = val_as_ch_i64(&ch.0)?;
            textbox_insert_printable_char(_interp, self_val.0, code)?;
            Ok(Value::none())
        }

        fn do_command(#[positional_only] self_val: PyValueRef<'_>, ch: PyValue) -> Result<Value> {
            let (maxy, maxx) = textbox_update_max_yx(_interp, self_val.0)?;
            let win = _interp.get_attr(self_val.0, "win")?;
            let yx = call_method(_interp, &win, "getyx", &[])?;
            let (y, x) = unpack_pair_i64(_interp, &yx)?;
            _interp.assign_attr(self_val.0.clone(), "lastcmd", ch.0.clone())?;

            let code = val_as_ch_i64(&ch.0)?;
            let strip_val = _interp.get_attr(self_val.0, "stripspaces")?;
            let stripspaces = _interp.truthy_value(&strip_val)?;

            if (32..=126).contains(&code) {
                if y < maxy || x < maxx {
                    textbox_insert_printable_char(_interp, self_val.0, code)?;
                }
            } else if code == 0x01 {
                // Ctrl-A
                call_method(_interp, &win, "move", &[Value::int(y), Value::int(0)])?;
            } else if code == 0x02
                || code == eng::KEY_LEFT
                || code == 0x08
                || code == eng::KEY_BACKSPACE
                || code == 0x7f
            {
                if x > 0 {
                    call_method(_interp, &win, "move", &[Value::int(y), Value::int(x - 1)])?;
                } else if y == 0 {
                    // At origin; do nothing
                } else if stripspaces {
                    let eol = textbox_end_of_line(_interp, self_val.0, y - 1)?;
                    call_method(_interp, &win, "move", &[Value::int(y - 1), Value::int(eol)])?;
                } else {
                    call_method(
                        _interp,
                        &win,
                        "move",
                        &[Value::int(y - 1), Value::int(maxx)],
                    )?;
                }
                if code == 0x08 || code == eng::KEY_BACKSPACE || code == 0x7f {
                    call_method(_interp, &win, "delch", &[])?;
                }
            } else if code == 0x04 {
                // Ctrl-D
                call_method(_interp, &win, "delch", &[])?;
            } else if code == 0x05 {
                // Ctrl-E
                if stripspaces {
                    let eol = textbox_end_of_line(_interp, self_val.0, y)?;
                    call_method(_interp, &win, "move", &[Value::int(y), Value::int(eol)])?;
                } else {
                    call_method(_interp, &win, "move", &[Value::int(y), Value::int(maxx)])?;
                }
            } else if code == 0x06 || code == eng::KEY_RIGHT {
                // Ctrl-F / Right
                if x < maxx {
                    call_method(_interp, &win, "move", &[Value::int(y), Value::int(x + 1)])?;
                } else if y < maxy {
                    call_method(_interp, &win, "move", &[Value::int(y + 1), Value::int(0)])?;
                }
            } else if code == 0x07 {
                // Ctrl-G: terminate
                return Ok(Value::int(0));
            } else if code == 0x0a {
                // Ctrl-J / NL
                if maxy == 0 {
                    return Ok(Value::int(0));
                } else if y < maxy {
                    call_method(_interp, &win, "move", &[Value::int(y + 1), Value::int(0)])?;
                }
            } else if code == 0x0b {
                // Ctrl-K
                if x == 0 && textbox_end_of_line(_interp, self_val.0, y)? == 0 {
                    call_method(_interp, &win, "deleteln", &[])?;
                } else {
                    call_method(_interp, &win, "move", &[Value::int(y), Value::int(x)])?;
                    call_method(_interp, &win, "clrtoeol", &[])?;
                }
            } else if code == 0x0c {
                // Ctrl-L
                call_method(_interp, &win, "refresh", &[])?;
            } else if code == 0x0e || code == eng::KEY_DOWN {
                // Ctrl-N / Down
                if y < maxy {
                    call_method(_interp, &win, "move", &[Value::int(y + 1), Value::int(x)])?;
                    let eol = textbox_end_of_line(_interp, self_val.0, y + 1)?;
                    if x > eol {
                        call_method(_interp, &win, "move", &[Value::int(y + 1), Value::int(eol)])?;
                    }
                }
            } else if code == 0x0f {
                // Ctrl-O
                call_method(_interp, &win, "insertln", &[])?;
            } else if code == 0x10 || code == eng::KEY_UP {
                // Ctrl-P / Up
                if y > 0 {
                    call_method(_interp, &win, "move", &[Value::int(y - 1), Value::int(x)])?;
                    let eol = textbox_end_of_line(_interp, self_val.0, y - 1)?;
                    if x > eol {
                        call_method(_interp, &win, "move", &[Value::int(y - 1), Value::int(eol)])?;
                    }
                }
            }
            Ok(Value::int(1))
        }

        fn gather(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (maxy, maxx) = textbox_update_max_yx(_interp, self_val.0)?;
            let win = _interp.get_attr(self_val.0, "win")?;
            let strip_val = _interp.get_attr(self_val.0, "stripspaces")?;
            let stripspaces = _interp.truthy_value(&strip_val)?;
            let mut result = String::new();
            for y in 0..=maxy {
                call_method(_interp, &win, "move", &[Value::int(y), Value::int(0)])?;
                let stop = textbox_end_of_line(_interp, self_val.0, y)?;
                if stop == 0 && stripspaces {
                    continue;
                }
                for x in 0..=maxx {
                    if stripspaces && x > stop {
                        break;
                    }
                    let ch_val =
                        call_method(_interp, &win, "inch", &[Value::int(y), Value::int(x)])?;
                    let c = (val_as_ch_i64(&ch_val)? & 0x7f) as u8 as char;
                    result.push(c);
                }
                if maxy > 0 {
                    result.push('\n');
                }
            }
            Ok(Value::string(result))
        }

        fn edit(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(None)] validate: Option<PyCallable>,
        ) -> Result<Value> {
            let win = _interp.get_attr(self_val.0, "win")?;
            loop {
                let mut ch = call_method(_interp, &win, "getch", &[])?;
                if let Some(ref val_fn) = validate {
                    ch = call_pos(_interp, val_fn.as_value().clone(), &[ch])?;
                }
                if !_interp.truthy_value(&ch)? {
                    continue;
                }
                let keep_going = call_method(_interp, self_val.0, "do_command", &[ch])?;
                if !_interp.truthy_value(&keep_going)? {
                    break;
                }
                call_method(_interp, &win, "refresh", &[])?;
            }
            call_method(_interp, self_val.0, "gather", &[])
        }
    }
}
