// `_curses` module — 100% native Rust implementation of Python 3.12's `_curses`
// C extension backed by `curses_engine` (zero `libncurses` linkage) (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.
// Reference: <https://docs.python.org/3.12/library/curses.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::curses_engine::{self as eng, Cell};
use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyStr, PyValue,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

pub(crate) struct WindowHandle {
    pub(crate) id: usize,
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

pub(crate) fn raise_curses_error(interp: &mut Interpreter, msg: impl Into<String>) -> PyError {
    let msg_str = msg.into();
    if let Ok(m) = interp.load_module("_curses")
        && let Ok(cls) = interp.get_attr(&m, "error")
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(&msg_str)])
    {
        return PyError::raised(inst);
    }
    PyError::named("error", msg_str)
}

pub(crate) fn make_window_value(interp: &mut Interpreter, win_id: usize) -> Result<Value> {
    let mod_val = interp.load_module("_curses")?;
    let cls_val = interp.get_attr(&mod_val, "window")?;
    let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
        return Err(PyError::runtime(
            "_curses.window is not a class".to_string(),
        ));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::default(),
    })));
    set_native_state(&inst, WindowHandle { id: win_id })?;
    Ok(inst)
}

pub(crate) fn extract_window_id(val: &Value) -> Result<usize> {
    with_native_state::<WindowHandle, usize>(val, "_curses.window", |h| Ok(h.id))
}

/// Publish `LINES`, `COLS`, and `ACS_*` constants on `_curses` (and on `curses`
/// if already imported).
pub(crate) fn publish_screen_constants(interp: &mut Interpreter) {
    let (lines, cols) = eng::with_state(|st| (st.lines as i64, st.cols as i64));
    if let Ok(m) = interp.load_module("_curses")
        && let ValueKind::PyModule(rc) = m.kind()
    {
        let mut bm = rc.borrow_mut();
        bm.insert_attr("LINES".to_string(), Value::int(lines));
        bm.insert_attr("COLS".to_string(), Value::int(cols));
        for &(name, val) in eng::ACS_TABLE {
            bm.insert_attr(name.to_string(), Value::int(val as i64));
        }
    }
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(modules) = interp.get_attr(&sys_mod, "modules")
        && let Some(dict) = modules.as_dict()
        && let Some(curses_mod) = dict.get(&crate::value::PyKey::str_from("curses"))
        && let ValueKind::PyModule(rc) = curses_mod.kind()
    {
        let mut bm = rc.borrow_mut();
        bm.insert_attr("LINES".to_string(), Value::int(lines));
        bm.insert_attr("COLS".to_string(), Value::int(cols));
        for &(name, val) in eng::ACS_TABLE {
            bm.insert_attr(name.to_string(), Value::int(val as i64));
        }
    }
}

/// Publish `COLORS` and `COLOR_PAIRS` on `_curses` (and on `curses` if imported).
pub(crate) fn publish_color_constants(interp: &mut Interpreter) {
    let (colors, pairs) = eng::with_state(|st| (st.max_colors as i64, st.max_pairs as i64));
    if let Ok(m) = interp.load_module("_curses")
        && let ValueKind::PyModule(rc) = m.kind()
    {
        let mut bm = rc.borrow_mut();
        bm.insert_attr("COLORS".to_string(), Value::int(colors));
        bm.insert_attr("COLOR_PAIRS".to_string(), Value::int(pairs));
    }
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(modules) = interp.get_attr(&sys_mod, "modules")
        && let Some(dict) = modules.as_dict()
        && let Some(curses_mod) = dict.get(&crate::value::PyKey::str_from("curses"))
        && let ValueKind::PyModule(rc) = curses_mod.kind()
    {
        let mut bm = rc.borrow_mut();
        bm.insert_attr("COLORS".to_string(), Value::int(colors));
        bm.insert_attr("COLOR_PAIRS".to_string(), Value::int(pairs));
    }
}

fn require_initscr(interp: &mut Interpreter) -> Result<()> {
    if !eng::with_state(|st| st.initialized) {
        return Err(raise_curses_error(interp, "must call initscr() first"));
    }
    Ok(())
}

fn require_setupterm(interp: &mut Interpreter) -> Result<()> {
    if !eng::with_state(|st| st.setupterm_done || st.initialized) {
        return Err(raise_curses_error(
            interp,
            "must call (at least) setupterm() first",
        ));
    }
    Ok(())
}

fn as_i64_arg(val: &Value, name: &str) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "integer argument expected for '{name}', got '{}'",
                value_type_name_str(val)
            ),
        )),
    }
}

fn as_u32_attr(val: &Value, name: &str) -> Result<u32> {
    let n = as_i64_arg(val, name)?;
    Ok(n as u32)
}

/// Convert a curses `ch` argument (`int`, 1-char `str`, or 1-byte `bytes`) into
/// `(char, embedded_attr)`.
fn parse_ch_arg(val: &Value) -> Result<(char, u32)> {
    match val.kind() {
        ValueKind::Int(n) => {
            let u = n as u32;
            let attr = u & eng::A_ATTRIBUTES;
            let ch_bits = if attr == 0 && n > 255 {
                char::from_u32(u).unwrap_or(' ')
            } else {
                char::from_u32(u & 0xff).unwrap_or(' ')
            };
            Ok((ch_bits, attr))
        }
        ValueKind::Bool(b) => Ok((if b { '\x01' } else { '\0' }, 0)),
        ValueKind::Str(s) => {
            let mut chars = s.chars();
            let Some(c) = chars.next() else {
                return Err(PyError::named(
                    "TypeError",
                    "expect int or 1-character str or bytes".to_string(),
                ));
            };
            if chars.next().is_some() {
                return Err(PyError::named(
                    "TypeError",
                    "expect int or 1-character str or bytes".to_string(),
                ));
            }
            Ok((c, 0))
        }
        ValueKind::Bytes(b) => {
            if b.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    "expect int or 1-character str or bytes".to_string(),
                ));
            }
            Ok((b[0] as char, 0))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "expect int or 1-character str or bytes, got '{}'",
                value_type_name_str(val)
            ),
        )),
    }
}

fn parse_str_or_bytes_arg(val: &Value) -> Result<String> {
    match val.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        ValueKind::Bytes(b) => Ok(String::from_utf8_lossy(b).into_owned()),
        _ => Err(PyError::named(
            "TypeError",
            format!("expect str or bytes, got '{}'", value_type_name_str(val)),
        )),
    }
}

fn move_win_checked(interp: &mut Interpreter, win_id: usize, y: i64, x: i64) -> Result<()> {
    let ok = eng::with_state_mut(|st| {
        if let Some(win) = st.windows.get_mut(&win_id)
            && y >= 0
            && x >= 0
            && (y as usize) < win.height
            && (x as usize) < win.width
        {
            win.cur_y = y as usize;
            win.cur_x = x as usize;
            return true;
        }
        false
    });
    if !ok {
        return Err(raise_curses_error(interp, "wmove() returned ERR"));
    }
    Ok(())
}

fn win_add_string(
    interp: &mut Interpreter,
    win_id: usize,
    text: &str,
    max_chars: Option<i64>,
    attr: u32,
) -> Result<()> {
    let limit = match max_chars {
        Some(n) if n >= 0 => Some(n as usize),
        _ => None,
    };
    for (written, ch) in text.chars().enumerate() {
        if let Some(lim) = limit
            && written >= lim
        {
            break;
        }
        let res = eng::with_state_mut(|st| st.win_put_char(win_id, ch, attr));
        if let Err(msg) = res {
            return Err(raise_curses_error(interp, msg));
        }
    }
    Ok(())
}

fn do_ungetch(interp: &mut Interpreter, ch: PyValue) -> Result<Value> {
    require_initscr(interp)?;
    let code = match ch.0.kind() {
        ValueKind::Int(n) => n as i32,
        ValueKind::Str(s) => {
            let mut cs = s.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => c as i32,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "expect int or 1-character str or bytes".to_string(),
                    ));
                }
            }
        }
        ValueKind::Bytes(b) if b.len() == 1 => b[0] as i32,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "expect int or 1-character str or bytes".to_string(),
            ));
        }
    };
    eng::with_state_mut(|st| st.ungetch_buf.push_front(code));
    Ok(Value::none())
}

fn do_resizeterm(interp: &mut Interpreter, nl: i64, nc: i64) -> Result<Value> {
    require_initscr(interp)?;
    if nl <= 0 || nc <= 0 {
        return Err(raise_curses_error(interp, "resizeterm() returned ERR"));
    }
    let nh = nl as usize;
    let nw = nc as usize;
    eng::with_state_mut(|st| {
        st.lines = nh;
        st.cols = nw;
        st.virtual_screen = vec![vec![Cell::default(); nw]; nh];
        if let Some(stdscr_id) = st.stdscr_id
            && let Some(w) = st.windows.get_mut(&stdscr_id)
        {
            let mut new_cells = vec![vec![w.bkgd; nw]; nh];
            let copy_w = nw.min(w.width);
            for (dst_row, src_row) in new_cells
                .iter_mut()
                .zip(w.cells.iter())
                .take(nh.min(w.height))
            {
                dst_row[..copy_w].copy_from_slice(&src_row[..copy_w]);
            }
            w.cells = new_cells;
            w.height = nh;
            w.width = nw;
            w.cur_y = w.cur_y.min(nh - 1);
            w.cur_x = w.cur_x.min(nw - 1);
            w.touched = vec![true; nh];
            w.scroll_top = 0;
            w.scroll_bot = nh - 1;
        }
    });
    publish_screen_constants(interp);
    Ok(Value::none())
}

pyrust_module! {
    constants {
        "version" => Value::bytes(b"2.2".to_vec()),
        "__version__" => Value::bytes(b"2.2".to_vec()),
        "ERR" => Value::int(eng::ERR),
        "OK" => Value::int(eng::OK),

        // Colors
        "COLOR_BLACK" => Value::int(eng::COLOR_BLACK),
        "COLOR_RED" => Value::int(eng::COLOR_RED),
        "COLOR_GREEN" => Value::int(eng::COLOR_GREEN),
        "COLOR_YELLOW" => Value::int(eng::COLOR_YELLOW),
        "COLOR_BLUE" => Value::int(eng::COLOR_BLUE),
        "COLOR_MAGENTA" => Value::int(eng::COLOR_MAGENTA),
        "COLOR_CYAN" => Value::int(eng::COLOR_CYAN),
        "COLOR_WHITE" => Value::int(eng::COLOR_WHITE),

        // Attributes
        "A_NORMAL" => Value::int(eng::A_NORMAL as i64),
        "A_CHARTEXT" => Value::int(eng::A_CHARTEXT as i64),
        "A_COLOR" => Value::int(eng::A_COLOR as i64),
        "A_STANDOUT" => Value::int(eng::A_STANDOUT as i64),
        "A_UNDERLINE" => Value::int(eng::A_UNDERLINE as i64),
        "A_REVERSE" => Value::int(eng::A_REVERSE as i64),
        "A_BLINK" => Value::int(eng::A_BLINK as i64),
        "A_DIM" => Value::int(eng::A_DIM as i64),
        "A_BOLD" => Value::int(eng::A_BOLD as i64),
        "A_ALTCHARSET" => Value::int(eng::A_ALTCHARSET as i64),
        "A_INVIS" => Value::int(eng::A_INVIS as i64),
        "A_PROTECT" => Value::int(eng::A_PROTECT as i64),
        "A_HORIZONTAL" => Value::int(eng::A_HORIZONTAL as i64),
        "A_LEFT" => Value::int(eng::A_LEFT as i64),
        "A_LOW" => Value::int(eng::A_LOW as i64),
        "A_RIGHT" => Value::int(eng::A_RIGHT as i64),
        "A_TOP" => Value::int(eng::A_TOP as i64),
        "A_VERTICAL" => Value::int(eng::A_VERTICAL as i64),
        "A_ITALIC" => Value::int(eng::A_ITALIC as i64),
        "A_ATTRIBUTES" => Value::int(eng::A_ATTRIBUTES as i64),

        // Mouse constants
        "BUTTON1_RELEASED" => Value::int(eng::BUTTON1_RELEASED),
        "BUTTON1_PRESSED" => Value::int(eng::BUTTON1_PRESSED),
        "BUTTON1_CLICKED" => Value::int(eng::BUTTON1_CLICKED),
        "BUTTON1_DOUBLE_CLICKED" => Value::int(eng::BUTTON1_DOUBLE_CLICKED),
        "BUTTON1_TRIPLE_CLICKED" => Value::int(eng::BUTTON1_TRIPLE_CLICKED),
        "BUTTON2_RELEASED" => Value::int(eng::BUTTON2_RELEASED),
        "BUTTON2_PRESSED" => Value::int(eng::BUTTON2_PRESSED),
        "BUTTON2_CLICKED" => Value::int(eng::BUTTON2_CLICKED),
        "BUTTON2_DOUBLE_CLICKED" => Value::int(eng::BUTTON2_DOUBLE_CLICKED),
        "BUTTON2_TRIPLE_CLICKED" => Value::int(eng::BUTTON2_TRIPLE_CLICKED),
        "BUTTON3_RELEASED" => Value::int(eng::BUTTON3_RELEASED),
        "BUTTON3_PRESSED" => Value::int(eng::BUTTON3_PRESSED),
        "BUTTON3_CLICKED" => Value::int(eng::BUTTON3_CLICKED),
        "BUTTON3_DOUBLE_CLICKED" => Value::int(eng::BUTTON3_DOUBLE_CLICKED),
        "BUTTON3_TRIPLE_CLICKED" => Value::int(eng::BUTTON3_TRIPLE_CLICKED),
        "BUTTON4_RELEASED" => Value::int(eng::BUTTON4_RELEASED),
        "BUTTON4_PRESSED" => Value::int(eng::BUTTON4_PRESSED),
        "BUTTON4_CLICKED" => Value::int(eng::BUTTON4_CLICKED),
        "BUTTON4_DOUBLE_CLICKED" => Value::int(eng::BUTTON4_DOUBLE_CLICKED),
        "BUTTON4_TRIPLE_CLICKED" => Value::int(eng::BUTTON4_TRIPLE_CLICKED),
        "BUTTON_SHIFT" => Value::int(eng::BUTTON_SHIFT),
        "BUTTON_CTRL" => Value::int(eng::BUTTON_CTRL),
        "BUTTON_ALT" => Value::int(eng::BUTTON_ALT),
        "ALL_MOUSE_EVENTS" => Value::int(eng::ALL_MOUSE_EVENTS),
        "REPORT_MOUSE_POSITION" => Value::int(eng::REPORT_MOUSE_POSITION),

        // Key constants
        "KEY_MIN" => Value::int(eng::KEY_MIN),
        "KEY_BREAK" => Value::int(eng::KEY_BREAK),
        "KEY_DOWN" => Value::int(eng::KEY_DOWN),
        "KEY_UP" => Value::int(eng::KEY_UP),
        "KEY_LEFT" => Value::int(eng::KEY_LEFT),
        "KEY_RIGHT" => Value::int(eng::KEY_RIGHT),
        "KEY_HOME" => Value::int(eng::KEY_HOME),
        "KEY_BACKSPACE" => Value::int(eng::KEY_BACKSPACE),
        "KEY_F0" => Value::int(eng::KEY_F0),
        "KEY_F1" => Value::int(eng::KEY_F0 + 1),
        "KEY_F2" => Value::int(eng::KEY_F0 + 2),
        "KEY_F3" => Value::int(eng::KEY_F0 + 3),
        "KEY_F4" => Value::int(eng::KEY_F0 + 4),
        "KEY_F5" => Value::int(eng::KEY_F0 + 5),
        "KEY_F6" => Value::int(eng::KEY_F0 + 6),
        "KEY_F7" => Value::int(eng::KEY_F0 + 7),
        "KEY_F8" => Value::int(eng::KEY_F0 + 8),
        "KEY_F9" => Value::int(eng::KEY_F0 + 9),
        "KEY_F10" => Value::int(eng::KEY_F0 + 10),
        "KEY_F11" => Value::int(eng::KEY_F0 + 11),
        "KEY_F12" => Value::int(eng::KEY_F0 + 12),
        "KEY_F13" => Value::int(eng::KEY_F0 + 13),
        "KEY_F14" => Value::int(eng::KEY_F0 + 14),
        "KEY_F15" => Value::int(eng::KEY_F0 + 15),
        "KEY_F16" => Value::int(eng::KEY_F0 + 16),
        "KEY_F17" => Value::int(eng::KEY_F0 + 17),
        "KEY_F18" => Value::int(eng::KEY_F0 + 18),
        "KEY_F19" => Value::int(eng::KEY_F0 + 19),
        "KEY_F20" => Value::int(eng::KEY_F0 + 20),
        "KEY_F21" => Value::int(eng::KEY_F0 + 21),
        "KEY_F22" => Value::int(eng::KEY_F0 + 22),
        "KEY_F23" => Value::int(eng::KEY_F0 + 23),
        "KEY_F24" => Value::int(eng::KEY_F0 + 24),
        "KEY_F25" => Value::int(eng::KEY_F0 + 25),
        "KEY_F26" => Value::int(eng::KEY_F0 + 26),
        "KEY_F27" => Value::int(eng::KEY_F0 + 27),
        "KEY_F28" => Value::int(eng::KEY_F0 + 28),
        "KEY_F29" => Value::int(eng::KEY_F0 + 29),
        "KEY_F30" => Value::int(eng::KEY_F0 + 30),
        "KEY_F31" => Value::int(eng::KEY_F0 + 31),
        "KEY_F32" => Value::int(eng::KEY_F0 + 32),
        "KEY_F33" => Value::int(eng::KEY_F0 + 33),
        "KEY_F34" => Value::int(eng::KEY_F0 + 34),
        "KEY_F35" => Value::int(eng::KEY_F0 + 35),
        "KEY_F36" => Value::int(eng::KEY_F0 + 36),
        "KEY_F37" => Value::int(eng::KEY_F0 + 37),
        "KEY_F38" => Value::int(eng::KEY_F0 + 38),
        "KEY_F39" => Value::int(eng::KEY_F0 + 39),
        "KEY_F40" => Value::int(eng::KEY_F0 + 40),
        "KEY_F41" => Value::int(eng::KEY_F0 + 41),
        "KEY_F42" => Value::int(eng::KEY_F0 + 42),
        "KEY_F43" => Value::int(eng::KEY_F0 + 43),
        "KEY_F44" => Value::int(eng::KEY_F0 + 44),
        "KEY_F45" => Value::int(eng::KEY_F0 + 45),
        "KEY_F46" => Value::int(eng::KEY_F0 + 46),
        "KEY_F47" => Value::int(eng::KEY_F0 + 47),
        "KEY_F48" => Value::int(eng::KEY_F0 + 48),
        "KEY_F49" => Value::int(eng::KEY_F0 + 49),
        "KEY_F50" => Value::int(eng::KEY_F0 + 50),
        "KEY_F51" => Value::int(eng::KEY_F0 + 51),
        "KEY_F52" => Value::int(eng::KEY_F0 + 52),
        "KEY_F53" => Value::int(eng::KEY_F0 + 53),
        "KEY_F54" => Value::int(eng::KEY_F0 + 54),
        "KEY_F55" => Value::int(eng::KEY_F0 + 55),
        "KEY_F56" => Value::int(eng::KEY_F0 + 56),
        "KEY_F57" => Value::int(eng::KEY_F0 + 57),
        "KEY_F58" => Value::int(eng::KEY_F0 + 58),
        "KEY_F59" => Value::int(eng::KEY_F0 + 59),
        "KEY_F60" => Value::int(eng::KEY_F0 + 60),
        "KEY_F61" => Value::int(eng::KEY_F0 + 61),
        "KEY_F62" => Value::int(eng::KEY_F0 + 62),
        "KEY_F63" => Value::int(eng::KEY_F0 + 63),
        "KEY_DL" => Value::int(eng::KEY_DL),
        "KEY_IL" => Value::int(eng::KEY_IL),
        "KEY_DC" => Value::int(eng::KEY_DC),
        "KEY_IC" => Value::int(eng::KEY_IC),
        "KEY_EIC" => Value::int(eng::KEY_EIC),
        "KEY_CLEAR" => Value::int(eng::KEY_CLEAR),
        "KEY_EOS" => Value::int(eng::KEY_EOS),
        "KEY_EOL" => Value::int(eng::KEY_EOL),
        "KEY_SF" => Value::int(eng::KEY_SF),
        "KEY_SR" => Value::int(eng::KEY_SR),
        "KEY_NPAGE" => Value::int(eng::KEY_NPAGE),
        "KEY_PPAGE" => Value::int(eng::KEY_PPAGE),
        "KEY_STAB" => Value::int(eng::KEY_STAB),
        "KEY_CTAB" => Value::int(eng::KEY_CTAB),
        "KEY_CATAB" => Value::int(eng::KEY_CATAB),
        "KEY_ENTER" => Value::int(eng::KEY_ENTER),
        "KEY_SRESET" => Value::int(eng::KEY_SRESET),
        "KEY_RESET" => Value::int(eng::KEY_RESET),
        "KEY_PRINT" => Value::int(eng::KEY_PRINT),
        "KEY_LL" => Value::int(eng::KEY_LL),
        "KEY_A1" => Value::int(eng::KEY_A1),
        "KEY_A3" => Value::int(eng::KEY_A3),
        "KEY_B2" => Value::int(eng::KEY_B2),
        "KEY_C1" => Value::int(eng::KEY_C1),
        "KEY_C3" => Value::int(eng::KEY_C3),
        "KEY_BTAB" => Value::int(eng::KEY_BTAB),
        "KEY_BEG" => Value::int(eng::KEY_BEG),
        "KEY_CANCEL" => Value::int(eng::KEY_CANCEL),
        "KEY_CLOSE" => Value::int(eng::KEY_CLOSE),
        "KEY_COMMAND" => Value::int(eng::KEY_COMMAND),
        "KEY_COPY" => Value::int(eng::KEY_COPY),
        "KEY_CREATE" => Value::int(eng::KEY_CREATE),
        "KEY_END" => Value::int(eng::KEY_END),
        "KEY_EXIT" => Value::int(eng::KEY_EXIT),
        "KEY_FIND" => Value::int(eng::KEY_FIND),
        "KEY_HELP" => Value::int(eng::KEY_HELP),
        "KEY_MARK" => Value::int(eng::KEY_MARK),
        "KEY_MESSAGE" => Value::int(eng::KEY_MESSAGE),
        "KEY_MOVE" => Value::int(eng::KEY_MOVE),
        "KEY_NEXT" => Value::int(eng::KEY_NEXT),
        "KEY_OPEN" => Value::int(eng::KEY_OPEN),
        "KEY_OPTIONS" => Value::int(eng::KEY_OPTIONS),
        "KEY_PREVIOUS" => Value::int(eng::KEY_PREVIOUS),
        "KEY_REDO" => Value::int(eng::KEY_REDO),
        "KEY_REFERENCE" => Value::int(eng::KEY_REFERENCE),
        "KEY_REFRESH" => Value::int(eng::KEY_REFRESH),
        "KEY_REPLACE" => Value::int(eng::KEY_REPLACE),
        "KEY_RESTART" => Value::int(eng::KEY_RESTART),
        "KEY_RESUME" => Value::int(eng::KEY_RESUME),
        "KEY_SAVE" => Value::int(eng::KEY_SAVE),
        "KEY_SBEG" => Value::int(eng::KEY_SBEG),
        "KEY_SCANCEL" => Value::int(eng::KEY_SCANCEL),
        "KEY_SCOMMAND" => Value::int(eng::KEY_SCOMMAND),
        "KEY_SCOPY" => Value::int(eng::KEY_SCOPY),
        "KEY_SCREATE" => Value::int(eng::KEY_SCREATE),
        "KEY_SDC" => Value::int(eng::KEY_SDC),
        "KEY_SDL" => Value::int(eng::KEY_SDL),
        "KEY_SELECT" => Value::int(eng::KEY_SELECT),
        "KEY_SEND" => Value::int(eng::KEY_SEND),
        "KEY_SEOL" => Value::int(eng::KEY_SEOL),
        "KEY_SEXIT" => Value::int(eng::KEY_SEXIT),
        "KEY_SFIND" => Value::int(eng::KEY_SFIND),
        "KEY_SHELP" => Value::int(eng::KEY_SHELP),
        "KEY_SHOME" => Value::int(eng::KEY_SHOME),
        "KEY_SIC" => Value::int(eng::KEY_SIC),
        "KEY_SLEFT" => Value::int(eng::KEY_SLEFT),
        "KEY_SMESSAGE" => Value::int(eng::KEY_SMESSAGE),
        "KEY_SMOVE" => Value::int(eng::KEY_SMOVE),
        "KEY_SNEXT" => Value::int(eng::KEY_SNEXT),
        "KEY_SOPTIONS" => Value::int(eng::KEY_SOPTIONS),
        "KEY_SPREVIOUS" => Value::int(eng::KEY_SPREVIOUS),
        "KEY_SPRINT" => Value::int(eng::KEY_SPRINT),
        "KEY_SREDO" => Value::int(eng::KEY_SREDO),
        "KEY_SREPLACE" => Value::int(eng::KEY_SREPLACE),
        "KEY_SRIGHT" => Value::int(eng::KEY_SRIGHT),
        "KEY_SRSUME" => Value::int(eng::KEY_SRSUME),
        "KEY_SSAVE" => Value::int(eng::KEY_SSAVE),
        "KEY_SSUSPEND" => Value::int(eng::KEY_SSUSPEND),
        "KEY_SUNDO" => Value::int(eng::KEY_SUNDO),
        "KEY_SUSPEND" => Value::int(eng::KEY_SUSPEND),
        "KEY_UNDO" => Value::int(eng::KEY_UNDO),
        "KEY_MOUSE" => Value::int(eng::KEY_MOUSE),
        "KEY_RESIZE" => Value::int(eng::KEY_RESIZE),
        "KEY_MAX" => Value::int(eng::KEY_MAX),
    }

    /// Exception raised when a curses library function returns an error.
    exception error;

    /// Curses window object backed by a pure-Rust virtual cell grid.
    #[native_state(WindowHandle)]
    class window {
        #[getter]
        fn encoding(#[state] h: &WindowHandle) -> Result<Value> {
            let enc = eng::with_state(|st| {
                st.windows
                    .get(&h.id)
                    .map(|w| w.encoding.clone())
                    .unwrap_or_else(|| "utf-8".to_string())
            });
            Ok(Value::string(enc))
        }

        #[setter("encoding")]
        fn set_encoding(#[state] h: &WindowHandle, val: PyStr<'_>) -> Result<Value> {
            let enc = val.0.to_string();
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.encoding = enc;
                }
            });
            Ok(Value::none())
        }

        #[py_name = "move"]
        fn py_move(#[state] h: &WindowHandle, new_y: PyIndex, new_x: PyIndex) -> Result<Value> {
            let y = new_y.expect_i64(FN_NAME, "new_y")?;
            let x = new_x.expect_i64(FN_NAME, "new_x")?;
            move_win_checked(_interp, h.id, y, x)?;
            Ok(Value::none())
        }

        fn getyx(#[state] h: &WindowHandle) -> Result<Value> {
            let (y, x) = eng::with_state(|st| {
                st.windows
                    .get(&h.id)
                    .map(|w| (w.cur_y as i64, w.cur_x as i64))
                    .unwrap_or((-1, -1))
            });
            Ok(Value::tuple(vec![Value::int(y), Value::int(x)]))
        }

        fn getparyx(#[state] h: &WindowHandle) -> Result<Value> {
            let (y, x) = eng::with_state(|st| match st.windows.get(&h.id) {
                Some(w) if w.parent_id.is_some() => (w.par_y as i64, w.par_x as i64),
                _ => (-1, -1),
            });
            Ok(Value::tuple(vec![Value::int(y), Value::int(x)]))
        }

        fn getbegyx(#[state] h: &WindowHandle) -> Result<Value> {
            let (y, x) = eng::with_state(|st| {
                st.windows
                    .get(&h.id)
                    .map(|w| (w.beg_y as i64, w.beg_x as i64))
                    .unwrap_or((-1, -1))
            });
            Ok(Value::tuple(vec![Value::int(y), Value::int(x)]))
        }

        fn getmaxyx(#[state] h: &WindowHandle) -> Result<Value> {
            let (h_val, w_val) = eng::with_state(|st| {
                st.windows
                    .get(&h.id)
                    .map(|w| (w.height as i64, w.width as i64))
                    .unwrap_or((-1, -1))
            });
            Ok(Value::tuple(vec![Value::int(h_val), Value::int(w_val)]))
        }

        fn addstr(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (text, attr) = match args.len() {
                1 => (parse_str_or_bytes_arg(&args[0])?, 0u32),
                2 => (
                    parse_str_or_bytes_arg(&args[0])?,
                    as_u32_attr(&args[1], "attr")?,
                ),
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_str_or_bytes_arg(&args[2])?, 0u32)
                }
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        parse_str_or_bytes_arg(&args[2])?,
                        as_u32_attr(&args[3], "attr")?,
                    )
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "addstr requires 1 to 4 arguments".to_string(),
                    ));
                }
            };
            win_add_string(_interp, h.id, &text, None, attr)?;
            Ok(Value::none())
        }

        fn addnstr(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (text, n, attr) = match args.len() {
                2 => (
                    parse_str_or_bytes_arg(&args[0])?,
                    as_i64_arg(&args[1], "n")?,
                    0u32,
                ),
                3 => (
                    parse_str_or_bytes_arg(&args[0])?,
                    as_i64_arg(&args[1], "n")?,
                    as_u32_attr(&args[2], "attr")?,
                ),
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        parse_str_or_bytes_arg(&args[2])?,
                        as_i64_arg(&args[3], "n")?,
                        0u32,
                    )
                }
                5 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        parse_str_or_bytes_arg(&args[2])?,
                        as_i64_arg(&args[3], "n")?,
                        as_u32_attr(&args[4], "attr")?,
                    )
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "addnstr requires 2 to 5 arguments".to_string(),
                    ));
                }
            };
            win_add_string(_interp, h.id, &text, Some(n), attr)?;
            Ok(Value::none())
        }

        fn addch(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let ((ch, emb_attr), extra_attr) = match args.len() {
                1 => (parse_ch_arg(&args[0])?, 0u32),
                2 => (parse_ch_arg(&args[0])?, as_u32_attr(&args[1], "attr")?),
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_ch_arg(&args[2])?, 0u32)
                }
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_ch_arg(&args[2])?, as_u32_attr(&args[3], "attr")?)
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "addch requires 1 to 4 arguments".to_string(),
                    ));
                }
            };
            let res = eng::with_state_mut(|st| st.win_put_char(h.id, ch, emb_attr | extra_attr));
            if let Err(msg) = res {
                return Err(raise_curses_error(_interp, msg));
            }
            Ok(Value::none())
        }

        fn insch(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let ((ch, emb_attr), extra_attr) = match args.len() {
                1 => (parse_ch_arg(&args[0])?, 0u32),
                2 => (parse_ch_arg(&args[0])?, as_u32_attr(&args[1], "attr")?),
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_ch_arg(&args[2])?, 0u32)
                }
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_ch_arg(&args[2])?, as_u32_attr(&args[3], "attr")?)
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "insch requires 1 to 4 arguments".to_string(),
                    ));
                }
            };
            eng::with_state_mut(|st| st.win_insch(h.id, ch, emb_attr | extra_attr));
            Ok(Value::none())
        }

        fn insstr(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (text, attr) = match args.len() {
                1 => (parse_str_or_bytes_arg(&args[0])?, 0u32),
                2 => (
                    parse_str_or_bytes_arg(&args[0])?,
                    as_u32_attr(&args[1], "attr")?,
                ),
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_str_or_bytes_arg(&args[2])?, 0u32)
                }
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        parse_str_or_bytes_arg(&args[2])?,
                        as_u32_attr(&args[3], "attr")?,
                    )
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "insstr requires 1 to 4 arguments".to_string(),
                    ));
                }
            };
            let chars: Vec<char> = text.chars().collect();
            eng::with_state_mut(|st| {
                for ch in chars.into_iter().rev() {
                    st.win_insch(h.id, ch, attr);
                }
            });
            Ok(Value::none())
        }

        fn delch(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            match args.len() {
                0 => {}
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "delch requires 0 or 2 arguments".to_string(),
                    ));
                }
            }
            eng::with_state_mut(|st| st.win_delch(h.id));
            Ok(Value::none())
        }

        fn deleteln(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| st.win_deleteln(h.id));
            Ok(Value::none())
        }

        fn insertln(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| st.win_insertln(h.id));
            Ok(Value::none())
        }

        fn inch(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            match args.len() {
                0 => {}
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "inch requires 0 or 2 arguments".to_string(),
                    ));
                }
            }
            let val = eng::with_state(|st| {
                let win = st.windows.get(&h.id)?;
                let cell = st.get_cell(h.id, win.cur_y, win.cur_x)?;
                Some(((cell.ch as u32) & 0xff) | cell.attr)
            })
            .unwrap_or(0);
            Ok(Value::int(val as i64))
        }

        fn instr(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let max_n: Option<usize> = match args.len() {
                0 => None,
                1 => {
                    let n = as_i64_arg(&args[0], "n")?;
                    if n < 0 { None } else { Some(n as usize) }
                }
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    None
                }
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    let n = as_i64_arg(&args[2], "n")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    if n < 0 { None } else { Some(n as usize) }
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "instr requires 0 to 3 arguments".to_string(),
                    ));
                }
            };
            let bytes = eng::with_state(|st| {
                let Some(win) = st.windows.get(&h.id) else {
                    return Vec::new();
                };
                let mut s = String::new();
                for x in win.cur_x..win.width {
                    if let Some(lim) = max_n
                        && s.len() >= lim
                    {
                        break;
                    }
                    let ch = st
                        .get_cell(h.id, win.cur_y, x)
                        .map(|c| c.ch)
                        .unwrap_or(' ');
                    s.push(ch);
                }
                let mut b = s.into_bytes();
                if let Some(lim) = max_n
                    && b.len() > lim
                {
                    b.truncate(lim);
                }
                b
            });
            Ok(Value::bytes(bytes))
        }

        fn clear(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| st.win_clear(h.id));
            Ok(Value::none())
        }

        fn erase(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| st.win_clear(h.id));
            Ok(Value::none())
        }

        fn clrtoeol(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| st.win_clrtoeol(h.id));
            Ok(Value::none())
        }

        fn clrtobot(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| st.win_clrtobot(h.id));
            Ok(Value::none())
        }

        #[py_name = "box"]
        fn py_box(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (v_spec, h_spec) = match args.len() {
                0 => ((char::from_u32(eng::ACS_VLINE & 0xff).unwrap(), eng::A_ALTCHARSET),
                      (char::from_u32(eng::ACS_HLINE & 0xff).unwrap(), eng::A_ALTCHARSET)),
                2 => {
                    let v = parse_ch_arg(&args[0])?;
                    let hr = parse_ch_arg(&args[1])?;
                    let v = if v.0 == '\0' && v.1 == 0 {
                        (char::from_u32(eng::ACS_VLINE & 0xff).unwrap(), eng::A_ALTCHARSET)
                    } else {
                        v
                    };
                    let hr = if hr.0 == '\0' && hr.1 == 0 {
                        (char::from_u32(eng::ACS_HLINE & 0xff).unwrap(), eng::A_ALTCHARSET)
                    } else {
                        hr
                    };
                    (v, hr)
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "box requires 0 or 2 arguments".to_string(),
                    ));
                }
            };
            eng::with_state_mut(|st| {
                let Some((height, width, attrs)) = st
                    .windows
                    .get(&h.id)
                    .map(|w| (w.height, w.width, w.attrs))
                else {
                    return;
                };
                if height == 0 || width == 0 {
                    return;
                }
                let tl = Cell { ch: (eng::ACS_ULCORNER & 0xff) as u8 as char, attr: eng::A_ALTCHARSET | attrs };
                let tr = Cell { ch: (eng::ACS_URCORNER & 0xff) as u8 as char, attr: eng::A_ALTCHARSET | attrs };
                let bl = Cell { ch: (eng::ACS_LLCORNER & 0xff) as u8 as char, attr: eng::A_ALTCHARSET | attrs };
                let br = Cell { ch: (eng::ACS_LRCORNER & 0xff) as u8 as char, attr: eng::A_ALTCHARSET | attrs };
                let hc = Cell { ch: h_spec.0, attr: h_spec.1 | attrs };
                let vc = Cell { ch: v_spec.0, attr: v_spec.1 | attrs };

                for x in 1..width.saturating_sub(1) {
                    st.set_cell(h.id, 0, x, hc);
                    st.set_cell(h.id, height - 1, x, hc);
                }
                for y in 1..height.saturating_sub(1) {
                    st.set_cell(h.id, y, 0, vc);
                    st.set_cell(h.id, y, width - 1, vc);
                }
                st.set_cell(h.id, 0, 0, tl);
                if width > 1 {
                    st.set_cell(h.id, 0, width - 1, tr);
                }
                if height > 1 {
                    st.set_cell(h.id, height - 1, 0, bl);
                    if width > 1 {
                        st.set_cell(h.id, height - 1, width - 1, br);
                    }
                }
            });
            Ok(Value::none())
        }

        fn border(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            if args.len() > 8 {
                return Err(PyError::named(
                    "TypeError",
                    "border requires at most 8 arguments".to_string(),
                ));
            }
            let defaults = [
                eng::ACS_VLINE,
                eng::ACS_VLINE,
                eng::ACS_HLINE,
                eng::ACS_HLINE,
                eng::ACS_ULCORNER,
                eng::ACS_URCORNER,
                eng::ACS_LLCORNER,
                eng::ACS_LRCORNER,
            ];
            let mut spec: [(char, u32); 8] = [(' ', 0); 8];
            for (idx, slot) in spec.iter_mut().enumerate() {
                if let Some(arg) = args.get(idx) {
                    let parsed = parse_ch_arg(arg)?;
                    if parsed.0 == '\0' && parsed.1 == 0 {
                        let d = defaults[idx];
                        *slot = ((d & 0xff) as u8 as char, d & eng::A_ATTRIBUTES);
                    } else {
                        *slot = parsed;
                    }
                } else {
                    let d = defaults[idx];
                    *slot = ((d & 0xff) as u8 as char, d & eng::A_ATTRIBUTES);
                }
            }
            eng::with_state_mut(|st| {
                let Some((height, width, attrs)) = st
                    .windows
                    .get(&h.id)
                    .map(|w| (w.height, w.width, w.attrs))
                else {
                    return;
                };
                if height == 0 || width == 0 {
                    return;
                }
                let mk = |p: (char, u32)| Cell { ch: p.0, attr: p.1 | attrs };
                for x in 1..width.saturating_sub(1) {
                    st.set_cell(h.id, 0, x, mk(spec[2]));
                    st.set_cell(h.id, height - 1, x, mk(spec[3]));
                }
                for y in 1..height.saturating_sub(1) {
                    st.set_cell(h.id, y, 0, mk(spec[0]));
                    st.set_cell(h.id, y, width - 1, mk(spec[1]));
                }
                st.set_cell(h.id, 0, 0, mk(spec[4]));
                if width > 1 {
                    st.set_cell(h.id, 0, width - 1, mk(spec[5]));
                }
                if height > 1 {
                    st.set_cell(h.id, height - 1, 0, mk(spec[6]));
                    if width > 1 {
                        st.set_cell(h.id, height - 1, width - 1, mk(spec[7]));
                    }
                }
            });
            Ok(Value::none())
        }

        fn hline(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let ((ch, emb), n, extra_attr) = match args.len() {
                2 => (parse_ch_arg(&args[0])?, as_i64_arg(&args[1], "n")?, 0u32),
                3 => (
                    parse_ch_arg(&args[0])?,
                    as_i64_arg(&args[1], "n")?,
                    as_u32_attr(&args[2], "attr")?,
                ),
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_ch_arg(&args[2])?, as_i64_arg(&args[3], "n")?, 0u32)
                }
                5 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        parse_ch_arg(&args[2])?,
                        as_i64_arg(&args[3], "n")?,
                        as_u32_attr(&args[4], "attr")?,
                    )
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "hline requires 2 to 5 arguments".to_string(),
                    ));
                }
            };
            if n > 0 {
                eng::with_state_mut(|st| {
                    let Some((cy, cx, w, win_attrs)) = st
                        .windows
                        .get(&h.id)
                        .map(|win| (win.cur_y, win.cur_x, win.width, win.attrs))
                    else {
                        return;
                    };
                    let cell = Cell {
                        ch,
                        attr: emb | extra_attr | win_attrs,
                    };
                    let end = (cx + n as usize).min(w);
                    for x in cx..end {
                        st.set_cell(h.id, cy, x, cell);
                    }
                });
            }
            Ok(Value::none())
        }

        fn vline(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let ((ch, emb), n, extra_attr) = match args.len() {
                2 => (parse_ch_arg(&args[0])?, as_i64_arg(&args[1], "n")?, 0u32),
                3 => (
                    parse_ch_arg(&args[0])?,
                    as_i64_arg(&args[1], "n")?,
                    as_u32_attr(&args[2], "attr")?,
                ),
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (parse_ch_arg(&args[2])?, as_i64_arg(&args[3], "n")?, 0u32)
                }
                5 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        parse_ch_arg(&args[2])?,
                        as_i64_arg(&args[3], "n")?,
                        as_u32_attr(&args[4], "attr")?,
                    )
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "vline requires 2 to 5 arguments".to_string(),
                    ));
                }
            };
            if n > 0 {
                eng::with_state_mut(|st| {
                    let Some((cy, cx, height, win_attrs)) = st
                        .windows
                        .get(&h.id)
                        .map(|win| (win.cur_y, win.cur_x, win.height, win.attrs))
                    else {
                        return;
                    };
                    let cell = Cell {
                        ch,
                        attr: emb | extra_attr | win_attrs,
                    };
                    let end = (cy + n as usize).min(height);
                    for y in cy..end {
                        st.set_cell(h.id, y, cx, cell);
                    }
                });
            }
            Ok(Value::none())
        }

        fn bkgdset(
            #[state] h: &WindowHandle,
            ch: PyValue,
            #[default(OptionalArg::Omitted)] attr: OptValueRef<'_>,
        ) -> Result<Value> {
            let (c, emb) = parse_ch_arg(&ch.0)?;
            let extra = match attr {
                OptionalArg::Provided(v) => as_u32_attr(v.0, "attr")?,
                OptionalArg::Omitted => 0,
            };
            let bg = Cell {
                ch: if c == '\0' { ' ' } else { c },
                attr: emb | extra,
            };
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.bkgd = bg;
                }
            });
            Ok(Value::none())
        }

        fn bkgd(
            #[state] h: &WindowHandle,
            ch: PyValue,
            #[default(OptionalArg::Omitted)] attr: OptValueRef<'_>,
        ) -> Result<Value> {
            let (c, emb) = parse_ch_arg(&ch.0)?;
            let extra = match attr {
                OptionalArg::Provided(v) => as_u32_attr(v.0, "attr")?,
                OptionalArg::Omitted => 0,
            };
            let bg = Cell {
                ch: if c == '\0' { ' ' } else { c },
                attr: emb | extra,
            };
            eng::with_state_mut(|st| {
                let Some((height, width, old_bg)) = st
                    .windows
                    .get(&h.id)
                    .map(|w| (w.height, w.width, w.bkgd))
                else {
                    return;
                };
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.bkgd = bg;
                }
                for y in 0..height {
                    for x in 0..width {
                        if let Some(mut cell) = st.get_cell(h.id, y, x) {
                            if cell.ch == old_bg.ch {
                                cell.ch = bg.ch;
                            }
                            cell.attr = (cell.attr & !old_bg.attr) | bg.attr;
                            st.set_cell(h.id, y, x, cell);
                        }
                    }
                }
            });
            Ok(Value::none())
        }

        fn attron(#[state] h: &WindowHandle, attr: PyIndex) -> Result<Value> {
            let a = attr.expect_i64(FN_NAME, "attr")? as u32;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    if (a & eng::A_COLOR) != 0 {
                        w.attrs = (w.attrs & !eng::A_COLOR) | a;
                    } else {
                        w.attrs |= a;
                    }
                }
            });
            Ok(Value::none())
        }

        fn attroff(#[state] h: &WindowHandle, attr: PyIndex) -> Result<Value> {
            let a = attr.expect_i64(FN_NAME, "attr")? as u32;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.attrs &= !a;
                }
            });
            Ok(Value::none())
        }

        fn attrset(#[state] h: &WindowHandle, attr: PyIndex) -> Result<Value> {
            let a = attr.expect_i64(FN_NAME, "attr")? as u32;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.attrs = a;
                }
            });
            Ok(Value::none())
        }

        fn attr_get(#[state] h: &WindowHandle) -> Result<Value> {
            let attrs = eng::with_state(|st| {
                st.windows.get(&h.id).map(|w| w.attrs).unwrap_or(0)
            });
            Ok(Value::tuple(vec![
                Value::int(attrs as i64),
                Value::int(eng::pair_number(attrs)),
            ]))
        }

        fn chgat(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (num, attr) = match args.len() {
                1 => (-1i64, as_u32_attr(&args[0], "attr")?),
                2 => (
                    as_i64_arg(&args[0], "num")?,
                    as_u32_attr(&args[1], "attr")?,
                ),
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (-1i64, as_u32_attr(&args[2], "attr")?)
                }
                4 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    (
                        as_i64_arg(&args[2], "num")?,
                        as_u32_attr(&args[3], "attr")?,
                    )
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "chgat requires 1 to 4 arguments".to_string(),
                    ));
                }
            };
            eng::with_state_mut(|st| {
                let Some((cy, cx, w)) = st
                    .windows
                    .get(&h.id)
                    .map(|win| (win.cur_y, win.cur_x, win.width))
                else {
                    return;
                };
                let end = if num < 0 {
                    w
                } else {
                    (cx + num as usize).min(w)
                };
                for x in cx..end {
                    if let Some(mut cell) = st.get_cell(h.id, cy, x) {
                        cell.attr = attr;
                        st.set_cell(h.id, cy, x, cell);
                    }
                }
            });
            Ok(Value::none())
        }

        fn noutrefresh(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let pad_rect = match args.len() {
                0 => None,
                6 => Some((
                    as_i64_arg(&args[0], "pminrow")?.max(0) as usize,
                    as_i64_arg(&args[1], "pmincol")?.max(0) as usize,
                    as_i64_arg(&args[2], "sminrow")?.max(0) as usize,
                    as_i64_arg(&args[3], "smincol")?.max(0) as usize,
                    as_i64_arg(&args[4], "smaxrow")?.max(0) as usize,
                    as_i64_arg(&args[5], "smaxcol")?.max(0) as usize,
                )),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "noutrefresh requires 0 or 6 arguments".to_string(),
                    ));
                }
            };
            let res = eng::with_state_mut(|st| st.win_noutrefresh(h.id, pad_rect));
            if let Err(msg) = res {
                return Err(raise_curses_error(_interp, msg));
            }
            Ok(Value::none())
        }

        fn refresh(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let pad_rect = match args.len() {
                0 => None,
                6 => Some((
                    as_i64_arg(&args[0], "pminrow")?.max(0) as usize,
                    as_i64_arg(&args[1], "pmincol")?.max(0) as usize,
                    as_i64_arg(&args[2], "sminrow")?.max(0) as usize,
                    as_i64_arg(&args[3], "smincol")?.max(0) as usize,
                    as_i64_arg(&args[4], "smaxrow")?.max(0) as usize,
                    as_i64_arg(&args[5], "smaxcol")?.max(0) as usize,
                )),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "refresh requires 0 or 6 arguments".to_string(),
                    ));
                }
            };
            let res: std::result::Result<(), String> = eng::with_state_mut(|st| {
                st.win_noutrefresh(h.id, pad_rect)?;
                st.doupdate();
                Ok(())
            });
            if let Err(msg) = res {
                return Err(raise_curses_error(_interp, msg));
            }
            Ok(Value::none())
        }

        fn redrawwin(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.touched.fill(true);
                }
            });
            Ok(Value::none())
        }

        fn touchwin(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.touched.fill(true);
                }
            });
            Ok(Value::none())
        }

        fn untouchwin(#[state] h: &WindowHandle) -> Result<Value> {
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.touched.fill(false);
                }
            });
            Ok(Value::none())
        }

        fn touchline(
            #[state] h: &WindowHandle,
            start: PyIndex,
            count: PyIndex,
            #[default(OptionalArg::Omitted)] changed: OptValueRef<'_>,
        ) -> Result<Value> {
            let s = start.expect_i64(FN_NAME, "start")?;
            let c = count.expect_i64(FN_NAME, "count")?;
            let flag = match changed {
                OptionalArg::Provided(v) => _interp.truthy_value(v.0)?,
                OptionalArg::Omitted => true,
            };
            if s < 0 || c < 0 {
                return Err(raise_curses_error(_interp, "touchline() returned ERR"));
            }
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    let start_idx = (s as usize).min(w.touched.len());
                    let end = ((s + c) as usize).min(w.touched.len());
                    w.touched[start_idx..end].fill(flag);
                }
            });
            Ok(Value::none())
        }

        fn is_wintouched(#[state] h: &WindowHandle) -> Result<Value> {
            let touched = eng::with_state(|st| {
                st.windows
                    .get(&h.id)
                    .is_some_and(|w| w.touched.iter().any(|&t| t))
            });
            Ok(Value::bool_(touched))
        }

        fn is_linetouched(#[state] h: &WindowHandle, line: PyIndex) -> Result<Value> {
            let ln = line.expect_i64(FN_NAME, "line")?;
            let res = eng::with_state(|st| {
                let w = st.windows.get(&h.id)?;
                if ln < 0 || (ln as usize) >= w.height {
                    return None;
                }
                Some(w.touched[ln as usize])
            });
            match res {
                Some(b) => Ok(Value::bool_(b)),
                None => Err(raise_curses_error(_interp, "is_linetouched: line out of range")),
            }
        }

        fn getch(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            match args.len() {
                0 => {}
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "getch requires 0 or 2 arguments".to_string(),
                    ));
                }
            }
            let ch = eng::with_state_mut(|st| st.read_key(h.id));
            Ok(Value::int(ch as i64))
        }

        fn getkey(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            match args.len() {
                0 => {}
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "getkey requires 0 or 2 arguments".to_string(),
                    ));
                }
            }
            let ch = eng::with_state_mut(|st| st.read_key(h.id));
            if ch < 0 {
                return Err(raise_curses_error(_interp, "no input"));
            }
            if ch >= 256 {
                let name = String::from_utf8_lossy(&eng::keyname_bytes(ch as i64)).into_owned();
                Ok(Value::string(name))
            } else {
                let c = char::from_u32(ch as u32).unwrap_or('\0');
                Ok(Value::string(c.to_string()))
            }
        }

        fn get_wch(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            match args.len() {
                0 => {}
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "get_wch requires 0 or 2 arguments".to_string(),
                    ));
                }
            }
            let ch = eng::with_state_mut(|st| st.read_key(h.id));
            if ch < 0 {
                return Err(raise_curses_error(_interp, "no input"));
            }
            if ch >= 256 {
                Ok(Value::int(ch as i64))
            } else {
                let c = char::from_u32(ch as u32).unwrap_or('\0');
                Ok(Value::string(c.to_string()))
            }
        }

        fn getstr(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let max_n: Option<usize> = match args.len() {
                0 => None,
                1 => {
                    let n = as_i64_arg(&args[0], "n")?;
                    if n < 0 { None } else { Some(n as usize) }
                }
                2 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    None
                }
                3 => {
                    let y = as_i64_arg(&args[0], "y")?;
                    let x = as_i64_arg(&args[1], "x")?;
                    let n = as_i64_arg(&args[2], "n")?;
                    move_win_checked(_interp, h.id, y, x)?;
                    if n < 0 { None } else { Some(n as usize) }
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "getstr requires 0 to 3 arguments".to_string(),
                    ));
                }
            };
            let mut out = Vec::new();
            loop {
                if let Some(lim) = max_n
                    && out.len() >= lim
                {
                    break;
                }
                let ch = eng::with_state_mut(|st| st.read_key(h.id));
                if ch < 0 || ch == b'\n' as i32 || ch == b'\r' as i32 {
                    break;
                }
                if ch < 256 {
                    out.push(ch as u8);
                }
            }
            Ok(Value::bytes(out))
        }

        fn nodelay(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.nodelay = v;
                }
            });
            Ok(Value::none())
        }

        fn timeout(#[state] h: &WindowHandle, delay: PyIndex) -> Result<Value> {
            let d = delay.expect_i64(FN_NAME, "delay")? as i32;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.timeout_ms = d;
                    w.nodelay = d == 0;
                }
            });
            Ok(Value::none())
        }

        fn keypad(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.keypad = v;
                }
            });
            Ok(Value::none())
        }

        fn leaveok(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.leaveok = v;
                }
            });
            Ok(Value::none())
        }

        fn scrollok(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.scrollok = v;
                }
            });
            Ok(Value::none())
        }

        fn idlok(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.idlok = v;
                }
            });
            Ok(Value::none())
        }

        fn immedok(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.immedok = v;
                }
            });
            Ok(Value::none())
        }

        fn syncok(#[state] h: &WindowHandle, flag: PyValue) -> Result<Value> {
            let v = _interp.truthy_value(&flag.0)?;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    w.syncok = v;
                }
            });
            Ok(Value::none())
        }

        fn setscrreg(#[state] h: &WindowHandle, top: PyIndex, bottom: PyIndex) -> Result<Value> {
            let t = top.expect_i64(FN_NAME, "top")?;
            let b = bottom.expect_i64(FN_NAME, "bottom")?;
            let ok = eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id)
                    && t >= 0
                    && b >= t
                    && (b as usize) < w.height
                {
                    w.scroll_top = t as usize;
                    w.scroll_bot = b as usize;
                    return true;
                }
                false
            });
            if !ok {
                return Err(raise_curses_error(_interp, "wsetscrreg() returned ERR"));
            }
            Ok(Value::none())
        }

        fn scroll(
            #[state] h: &WindowHandle,
            #[default(OptionalArg::Omitted)] lines: OptValueRef<'_>,
        ) -> Result<Value> {
            let n = match lines {
                OptionalArg::Provided(v) => as_i64_arg(v.0, "lines")? as i32,
                OptionalArg::Omitted => 1,
            };
            let res = eng::with_state_mut(|st| st.win_scroll(h.id, n));
            if let Err(msg) = res {
                return Err(raise_curses_error(_interp, msg));
            }
            Ok(Value::none())
        }

        fn mvwin(#[state] h: &WindowHandle, new_y: PyIndex, new_x: PyIndex) -> Result<Value> {
            let y = new_y.expect_i64(FN_NAME, "new_y")?;
            let x = new_x.expect_i64(FN_NAME, "new_x")?;
            let ok = eng::with_state_mut(|st| {
                let (lines, cols) = (st.lines, st.cols);
                if let Some(w) = st.windows.get_mut(&h.id)
                    && y >= 0
                    && x >= 0
                    && (y as usize) + w.height <= lines
                    && (x as usize) + w.width <= cols
                {
                    w.beg_y = y as usize;
                    w.beg_x = x as usize;
                    w.touched.fill(true);
                    return true;
                }
                false
            });
            if !ok {
                return Err(raise_curses_error(_interp, "mvwin() returned ERR"));
            }
            Ok(Value::none())
        }

        fn resize(#[state] h: &WindowHandle, nlines: PyIndex, ncols: PyIndex) -> Result<Value> {
            let nl = nlines.expect_i64(FN_NAME, "nlines")?;
            let nc = ncols.expect_i64(FN_NAME, "ncols")?;
            if nl <= 0 || nc <= 0 {
                return Err(raise_curses_error(_interp, "wresize() returned ERR"));
            }
            let nh = nl as usize;
            let nw = nc as usize;
            eng::with_state_mut(|st| {
                if let Some(w) = st.windows.get_mut(&h.id) {
                    if w.parent_id.is_none() {
                        let mut new_cells = vec![vec![w.bkgd; nw]; nh];
                        let copy_w = nw.min(w.width);
                        for (dst_row, src_row) in new_cells
                            .iter_mut()
                            .zip(w.cells.iter())
                            .take(nh.min(w.height))
                        {
                            dst_row[..copy_w].copy_from_slice(&src_row[..copy_w]);
                        }
                        w.cells = new_cells;
                    }
                    w.height = nh;
                    w.width = nw;
                    w.cur_y = w.cur_y.min(nh - 1);
                    w.cur_x = w.cur_x.min(nw - 1);
                    w.touched = vec![true; nh];
                    w.scroll_top = 0;
                    w.scroll_bot = nh - 1;
                }
            });
            Ok(Value::none())
        }

        fn subwin(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (nlines, ncols, begin_y, begin_x) = match args.len() {
                2 => (
                    0i64,
                    0i64,
                    as_i64_arg(&args[0], "begin_y")?,
                    as_i64_arg(&args[1], "begin_x")?,
                ),
                4 => (
                    as_i64_arg(&args[0], "nlines")?,
                    as_i64_arg(&args[1], "ncols")?,
                    as_i64_arg(&args[2], "begin_y")?,
                    as_i64_arg(&args[3], "begin_x")?,
                ),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "subwin requires 2 or 4 arguments".to_string(),
                    ));
                }
            };
            if nlines < 0 || ncols < 0 || begin_y < 0 || begin_x < 0 {
                return Err(raise_curses_error(_interp, "curses function returned NULL"));
            }
            let res = eng::with_state_mut(|st| {
                st.alloc_subwin(
                    h.id,
                    nlines as usize,
                    ncols as usize,
                    begin_y as usize,
                    begin_x as usize,
                    false,
                )
            });
            match res {
                Ok(id) => make_window_value(_interp, id),
                Err(msg) => Err(raise_curses_error(_interp, msg)),
            }
        }

        fn derwin(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            let (nlines, ncols, begin_y, begin_x) = match args.len() {
                2 => (
                    0i64,
                    0i64,
                    as_i64_arg(&args[0], "begin_y")?,
                    as_i64_arg(&args[1], "begin_x")?,
                ),
                4 => (
                    as_i64_arg(&args[0], "nlines")?,
                    as_i64_arg(&args[1], "ncols")?,
                    as_i64_arg(&args[2], "begin_y")?,
                    as_i64_arg(&args[3], "begin_x")?,
                ),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "derwin requires 2 or 4 arguments".to_string(),
                    ));
                }
            };
            if nlines < 0 || ncols < 0 || begin_y < 0 || begin_x < 0 {
                return Err(raise_curses_error(_interp, "curses function returned NULL"));
            }
            let res = eng::with_state_mut(|st| {
                st.alloc_subwin(
                    h.id,
                    nlines as usize,
                    ncols as usize,
                    begin_y as usize,
                    begin_x as usize,
                    true,
                )
            });
            match res {
                Ok(id) => make_window_value(_interp, id),
                Err(msg) => Err(raise_curses_error(_interp, msg)),
            }
        }

        fn overlay(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            copy_win_region(_interp, h.id, args, true)
        }

        fn overwrite(#[state] h: &WindowHandle, #[varargs] args: &[Value]) -> Result<Value> {
            copy_win_region(_interp, h.id, args, false)
        }
    }

    // ── Module-level functions ───────────────────────────────────────────────

    fn setupterm(
        #[default(None)] term: Option<PyStr<'_>>,
        #[default(OptionalArg::Omitted)] fd: OptValueRef<'_>,
    ) -> Result<Value> {
        let fd_val = match fd {
            OptionalArg::Provided(v) => as_i64_arg(v.0, "fd")? as i32,
            OptionalArg::Omitted => -1,
        };
        let term_ref = term.as_deref();
        let res = eng::with_state_mut(|st| st.setupterm(term_ref, fd_val));
        if let Err(msg) = res {
            return Err(raise_curses_error(_interp, msg));
        }
        Ok(Value::none())
    }

    fn initscr() -> Result<Value> {
        let res = eng::with_state_mut(|st| st.initscr());
        let id = match res {
            Ok(id) => id,
            Err(msg) => return Err(raise_curses_error(_interp, msg)),
        };
        publish_screen_constants(_interp);
        make_window_value(_interp, id)
    }

    fn endwin() -> Result<Value> {
        let res = eng::with_state_mut(|st| st.endwin());
        if let Err(msg) = res {
            return Err(raise_curses_error(_interp, msg));
        }
        Ok(Value::none())
    }

    fn isendwin() -> Result<Value> {
        require_initscr(_interp)?;
        let ended = eng::with_state(|st| st.ended);
        Ok(Value::bool_(ended))
    }

    fn newwin(#[varargs] args: &[Value]) -> Result<Value> {
        require_initscr(_interp)?;
        let (nlines, ncols, begin_y, begin_x) = match args.len() {
            2 => (
                as_i64_arg(&args[0], "nlines")?,
                as_i64_arg(&args[1], "ncols")?,
                0i64,
                0i64,
            ),
            4 => (
                as_i64_arg(&args[0], "nlines")?,
                as_i64_arg(&args[1], "ncols")?,
                as_i64_arg(&args[2], "begin_y")?,
                as_i64_arg(&args[3], "begin_x")?,
            ),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "newwin requires 2 or 4 arguments".to_string(),
                ));
            }
        };
        if nlines < 0 || ncols < 0 || begin_y < 0 || begin_x < 0 {
            return Err(raise_curses_error(_interp, "curses function returned NULL"));
        }
        let id = eng::with_state_mut(|st| {
            st.alloc_window(
                nlines as usize,
                ncols as usize,
                begin_y as usize,
                begin_x as usize,
                false,
            )
        });
        make_window_value(_interp, id)
    }

    fn newpad(nlines: PyIndex, ncols: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let nl = nlines.expect_i64(FN_NAME, "nlines")?;
        let nc = ncols.expect_i64(FN_NAME, "ncols")?;
        if nl <= 0 || nc <= 0 {
            return Err(raise_curses_error(_interp, "curses function returned NULL"));
        }
        let id = eng::with_state_mut(|st| st.alloc_window(nl as usize, nc as usize, 0, 0, true));
        make_window_value(_interp, id)
    }

    fn tigetflag(#[positional_only] capname: PyStr<'_>) -> Result<Value> {
        require_setupterm(_interp)?;
        let v = eng::with_state(|st| eng::tigetflag(&st.term, &capname.0));
        Ok(Value::int(v))
    }

    fn tigetnum(#[positional_only] capname: PyStr<'_>) -> Result<Value> {
        require_setupterm(_interp)?;
        let v = eng::with_state(|st| eng::tigetnum(&st.term, &capname.0, st.lines, st.cols));
        Ok(Value::int(v))
    }

    fn tigetstr(#[positional_only] capname: PyStr<'_>) -> Result<Value> {
        require_setupterm(_interp)?;
        let v = eng::with_state(|st| eng::tigetstr(&st.term, &capname.0));
        Ok(match v {
            Some(bytes) => Value::bytes(bytes.to_vec()),
            None => Value::none(),
        })
    }

    fn tparm(
        #[positional_only] fmt: PyBytesLike<'_>,
        #[varargs] args: &[Value],
    ) -> Result<Value> {
        require_setupterm(_interp)?;
        if args.len() > 9 {
            return Err(PyError::named(
                "TypeError",
                "tparm() takes at most 10 arguments".to_string(),
            ));
        }
        let mut params = Vec::with_capacity(args.len());
        for a in args {
            params.push(as_i64_arg(a, "param")?);
        }
        Ok(Value::bytes(eng::eval_tparm(fmt.as_bytes(), &params)))
    }

    fn cbreak(#[default(OptionalArg::Omitted)] flag: OptValueRef<'_>) -> Result<Value> {
        require_initscr(_interp)?;
        let on = match flag {
            OptionalArg::Provided(v) => _interp.truthy_value(v.0)?,
            OptionalArg::Omitted => true,
        };
        eng::with_state_mut(|st| {
            st.cbreak = on;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn nocbreak() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            st.cbreak = false;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn raw(#[default(OptionalArg::Omitted)] flag: OptValueRef<'_>) -> Result<Value> {
        require_initscr(_interp)?;
        let on = match flag {
            OptionalArg::Provided(v) => _interp.truthy_value(v.0)?,
            OptionalArg::Omitted => true,
        };
        eng::with_state_mut(|st| {
            st.raw = on;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn noraw() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            st.raw = false;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn echo(#[default(OptionalArg::Omitted)] flag: OptValueRef<'_>) -> Result<Value> {
        require_initscr(_interp)?;
        let on = match flag {
            OptionalArg::Provided(v) => _interp.truthy_value(v.0)?,
            OptionalArg::Omitted => true,
        };
        eng::with_state_mut(|st| {
            st.echo = on;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn noecho() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            st.echo = false;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn nl(#[default(OptionalArg::Omitted)] flag: OptValueRef<'_>) -> Result<Value> {
        require_initscr(_interp)?;
        let on = match flag {
            OptionalArg::Provided(v) => _interp.truthy_value(v.0)?,
            OptionalArg::Omitted => true,
        };
        eng::with_state_mut(|st| {
            st.nl = on;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn nonl() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            st.nl = false;
            st.apply_termios_modes();
        });
        Ok(Value::none())
    }

    fn curs_set(#[positional_only] visibility: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let v = visibility.expect_i64(FN_NAME, "visibility")? as i32;
        if !(0..=2).contains(&v) {
            return Err(raise_curses_error(_interp, "curs_set() returned ERR"));
        }
        let prev = eng::with_state_mut(|st| {
            let old = st.cursor_vis;
            st.cursor_vis = v;
            old
        });
        Ok(Value::int(prev as i64))
    }

    fn start_color() -> Result<Value> {
        require_initscr(_interp)?;
        let ok = eng::with_state_mut(|st| {
            if st.max_colors <= 0 {
                return false;
            }
            st.colors_started = true;
            true
        });
        if !ok {
            return Err(raise_curses_error(_interp, "start_color() returned ERR"));
        }
        publish_color_constants(_interp);
        Ok(Value::none())
    }

    fn use_default_colors() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            st.default_colors = true;
            st.pairs.insert(0, (-1, -1));
        });
        Ok(Value::none())
    }

    fn has_colors() -> Result<Value> {
        require_initscr(_interp)?;
        let has = eng::with_state(|st| st.max_colors > 0);
        Ok(Value::bool_(has))
    }

    fn can_change_color() -> Result<Value> {
        require_initscr(_interp)?;
        let can = eng::with_state(|st| eng::tigetflag(&st.term, "ccc") == 1);
        Ok(Value::bool_(can))
    }

    fn init_pair(
        #[positional_only] pair_number: PyIndex,
        #[positional_only] fg: PyIndex,
        #[positional_only] bg: PyIndex,
    ) -> Result<Value> {
        require_initscr(_interp)?;
        let pair = pair_number.expect_i64(FN_NAME, "pair_number")? as i32;
        let f = fg.expect_i64(FN_NAME, "fg")? as i32;
        let b = bg.expect_i64(FN_NAME, "bg")? as i32;
        let res = eng::with_state_mut(|st| {
            if !st.colors_started {
                return Err("start_color() must be called first");
            }
            if pair < 1 || pair >= st.max_pairs {
                return Err("Color pair is greater than COLOR_PAIRS-1");
            }
            let min_c = if st.default_colors { -1 } else { 0 };
            if f < min_c || f >= st.max_colors || b < min_c || b >= st.max_colors {
                return Err("Color number is out of range");
            }
            st.pairs.insert(pair, (f, b));
            Ok(())
        });
        if let Err(msg) = res {
            return Err(PyError::named("ValueError", msg.to_string()));
        }
        Ok(Value::none())
    }

    fn pair_content(#[positional_only] pair_number: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let pair = pair_number.expect_i64(FN_NAME, "pair_number")? as i32;
        let res = eng::with_state(|st| {
            if !st.colors_started || pair < 0 || pair >= st.max_pairs {
                return None;
            }
            Some(st.pairs.get(&pair).copied().unwrap_or((0, 0)))
        });
        match res {
            Some((fg, bg)) => Ok(Value::tuple(vec![
                Value::int(fg as i64),
                Value::int(bg as i64),
            ])),
            None => Err(raise_curses_error(_interp, "pair_content() returned ERR")),
        }
    }

    fn color_pair(#[positional_only] pair_number: PyIndex) -> Result<Value> {
        let n = pair_number.expect_i64(FN_NAME, "pair_number")?;
        Ok(Value::int(eng::color_pair(n) as i64))
    }

    fn pair_number(#[positional_only] attr: PyIndex) -> Result<Value> {
        let a = attr.expect_i64(FN_NAME, "attr")? as u32;
        Ok(Value::int(eng::pair_number(a)))
    }

    fn init_color(
        #[positional_only] color_number: PyIndex,
        #[positional_only] r: PyIndex,
        #[positional_only] g: PyIndex,
        #[positional_only] b: PyIndex,
    ) -> Result<Value> {
        require_initscr(_interp)?;
        let cn = color_number.expect_i64(FN_NAME, "color_number")? as i32;
        let rv = r.expect_i64(FN_NAME, "r")? as i32;
        let gv = g.expect_i64(FN_NAME, "g")? as i32;
        let bv = b.expect_i64(FN_NAME, "b")? as i32;
        let ok = eng::with_state_mut(|st| {
            if !st.colors_started
                || cn < 0
                || cn >= st.max_colors
                || !(0..=1000).contains(&rv)
                || !(0..=1000).contains(&gv)
                || !(0..=1000).contains(&bv)
            {
                return false;
            }
            st.palette.insert(cn, (rv, gv, bv));
            true
        });
        if !ok {
            return Err(raise_curses_error(_interp, "init_color() returned ERR"));
        }
        Ok(Value::none())
    }

    fn color_content(#[positional_only] color_number: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let cn = color_number.expect_i64(FN_NAME, "color_number")? as i32;
        let res = eng::with_state(|st| {
            if !st.colors_started || cn < 0 || cn >= st.max_colors {
                return None;
            }
            Some(st.palette.get(&cn).copied().unwrap_or((0, 0, 0)))
        });
        match res {
            Some((r, g, b)) => Ok(Value::tuple(vec![
                Value::int(r as i64),
                Value::int(g as i64),
                Value::int(b as i64),
            ])),
            None => Err(raise_curses_error(_interp, "color_content() returned ERR")),
        }
    }

    fn beep() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state(|st| {
            // SAFETY: `isatty` and `write` on fd.
            unsafe {
                if libc::isatty(st.out_fd) == 1 {
                    let _ = libc::write(st.out_fd, b"\x07".as_ptr().cast(), 1);
                }
            }
        });
        Ok(Value::none())
    }

    fn flash() -> Result<Value> {
        require_initscr(_interp)?;
        Ok(Value::none())
    }

    fn doupdate() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| st.doupdate());
        Ok(Value::none())
    }

    fn flushinp() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            st.ungetch_buf.clear();
            // SAFETY: `tcflush` fails cleanly on non-tty.
            unsafe {
                let _ = libc::tcflush(0, libc::TCIFLUSH);
            }
        });
        Ok(Value::none())
    }

    fn napms(#[positional_only] ms: PyIndex) -> Result<Value> {
        let m = ms.expect_i64(FN_NAME, "ms")?;
        if m > 0 {
            std::thread::sleep(std::time::Duration::from_millis(m as u64));
        }
        Ok(Value::int(m.max(0)))
    }

    fn ungetch(#[positional_only] ch: PyValue) -> Result<Value> {
        do_ungetch(_interp, ch)
    }

    fn unget_wch(#[positional_only] ch: PyValue) -> Result<Value> {
        do_ungetch(_interp, ch)
    }

    fn unctrl(#[positional_only] ch: PyValue) -> Result<Value> {
        require_initscr(_interp)?;
        let (c, _) = parse_ch_arg(&ch.0)?;
        Ok(Value::bytes(eng::unctrl_bytes(c as u32)))
    }

    fn keyname(#[positional_only] k: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let code = k.expect_i64(FN_NAME, "k")?;
        if code < 0 {
            return Err(PyError::named(
                "ValueError",
                "invalid key number".to_string(),
            ));
        }
        Ok(Value::bytes(eng::keyname_bytes(code)))
    }

    fn has_key(#[positional_only] key: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let k = key.expect_i64(FN_NAME, "key")?;
        let found = eng::with_state(|st| {
            eng::KEY_CAPABILITY_TABLE
                .iter()
                .find(|&&(code, _)| code == k)
                .and_then(|&(_, cap)| eng::tigetstr(&st.term, cap))
                .is_some()
        });
        Ok(Value::bool_(found))
    }

    fn has_ic() -> Result<Value> {
        require_initscr(_interp)?;
        let has = eng::with_state(|st| eng::tigetstr(&st.term, "ich1").is_some());
        Ok(Value::bool_(has))
    }

    fn has_il() -> Result<Value> {
        require_initscr(_interp)?;
        let has = eng::with_state(|st| eng::tigetstr(&st.term, "il1").is_some());
        Ok(Value::bool_(has))
    }

    fn baudrate() -> Result<Value> {
        require_initscr(_interp)?;
        Ok(Value::int(38400))
    }

    fn erasechar() -> Result<Value> {
        require_initscr(_interp)?;
        // SAFETY: `termios` is POD.
        let byte = unsafe {
            let mut mode: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut mode) == 0 {
                mode.c_cc[libc::VERASE]
            } else {
                0x7f
            }
        };
        Ok(Value::bytes(vec![byte]))
    }

    fn killchar() -> Result<Value> {
        require_initscr(_interp)?;
        // SAFETY: `termios` is POD.
        let byte = unsafe {
            let mut mode: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut mode) == 0 {
                mode.c_cc[libc::VKILL]
            } else {
                0x15
            }
        };
        Ok(Value::bytes(vec![byte]))
    }

    fn longname() -> Result<Value> {
        require_initscr(_interp)?;
        let name = eng::with_state(|st| st.term.clone());
        Ok(Value::bytes(name.into_bytes()))
    }

    fn termname() -> Result<Value> {
        require_initscr(_interp)?;
        let name = eng::with_state(|st| st.term.clone());
        Ok(Value::bytes(name.into_bytes()))
    }

    fn termattrs() -> Result<Value> {
        require_initscr(_interp)?;
        let attrs = eng::with_state(|st| {
            if st.term.eq_ignore_ascii_case("dumb") {
                eng::A_NORMAL
            } else {
                eng::A_BOLD | eng::A_UNDERLINE | eng::A_REVERSE | eng::A_STANDOUT | eng::A_DIM | eng::A_BLINK | eng::A_ALTCHARSET
            }
        });
        Ok(Value::int(attrs as i64))
    }

    fn def_prog_mode() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(0, &mut mode) } == 0 {
                st.prog_termios = Some(mode);
            }
        });
        Ok(Value::none())
    }

    fn def_shell_mode() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(0, &mut mode) } == 0 {
                st.shell_termios = Some(mode);
            }
        });
        Ok(Value::none())
    }

    fn reset_prog_mode() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state(|st| {
            if let Some(m) = st.prog_termios {
                unsafe {
                    let _ = libc::tcsetattr(0, libc::TCSANOW, &m);
                }
            }
        });
        Ok(Value::none())
    }

    fn reset_shell_mode() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state(|st| {
            if let Some(m) = st.shell_termios {
                unsafe {
                    let _ = libc::tcsetattr(0, libc::TCSANOW, &m);
                }
            }
        });
        Ok(Value::none())
    }

    fn savetty() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(0, &mut mode) } == 0 {
                st.saved_tty = Some(mode);
            }
        });
        Ok(Value::none())
    }

    fn resetty() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state(|st| {
            if let Some(m) = st.saved_tty {
                unsafe {
                    let _ = libc::tcsetattr(0, libc::TCSANOW, &m);
                }
            }
        });
        Ok(Value::none())
    }

    fn setsyx(#[positional_only] y: PyIndex, #[positional_only] x: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let yv = y.expect_i64(FN_NAME, "y")? as i32;
        let xv = x.expect_i64(FN_NAME, "x")? as i32;
        eng::with_state_mut(|st| {
            st.syx = (yv, xv);
        });
        Ok(Value::none())
    }

    fn getsyx() -> Result<Value> {
        require_initscr(_interp)?;
        let (y, x) = eng::with_state(|st| st.syx);
        Ok(Value::tuple(vec![Value::int(y as i64), Value::int(x as i64)]))
    }

    fn update_lines_cols() -> Result<Value> {
        require_initscr(_interp)?;
        eng::with_state_mut(|st| {
            let (lines, cols) = eng::query_terminal_size(st.out_fd);
            st.lines = lines;
            st.cols = cols;
        });
        publish_screen_constants(_interp);
        Ok(Value::none())
    }

    fn resizeterm(#[positional_only] nlines: PyIndex, #[positional_only] ncols: PyIndex) -> Result<Value> {
        let nl = nlines.expect_i64(FN_NAME, "nlines")?;
        let nc = ncols.expect_i64(FN_NAME, "ncols")?;
        do_resizeterm(_interp, nl, nc)
    }

    fn resize_term(#[positional_only] nlines: PyIndex, #[positional_only] ncols: PyIndex) -> Result<Value> {
        let nl = nlines.expect_i64(FN_NAME, "nlines")?;
        let nc = ncols.expect_i64(FN_NAME, "ncols")?;
        do_resizeterm(_interp, nl, nc)
    }

    fn is_term_resized(#[positional_only] nlines: PyIndex, #[positional_only] ncols: PyIndex) -> Result<Value> {
        require_initscr(_interp)?;
        let nl = nlines.expect_i64(FN_NAME, "nlines")?;
        let nc = ncols.expect_i64(FN_NAME, "ncols")?;
        let resized = eng::with_state(|st| nl != st.lines as i64 || nc != st.cols as i64);
        Ok(Value::bool_(resized))
    }
}

fn copy_win_region(
    interp: &mut Interpreter,
    src_id: usize,
    args: &[Value],
    skip_spaces: bool,
) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "overlay/overwrite requires at least 1 argument".to_string(),
        ));
    }
    let dst_id = extract_window_id(&args[0])?;
    match args.len() {
        1 => {
            eng::with_state_mut(|st| {
                let Some((sh, sw, sbeg_y, sbeg_x)) = st
                    .windows
                    .get(&src_id)
                    .map(|w| (w.height, w.width, w.beg_y, w.beg_x))
                else {
                    return;
                };
                let Some((dh, dw, dbeg_y, dbeg_x)) = st
                    .windows
                    .get(&dst_id)
                    .map(|w| (w.height, w.width, w.beg_y, w.beg_x))
                else {
                    return;
                };
                let top = sbeg_y.max(dbeg_y);
                let left = sbeg_x.max(dbeg_x);
                let bot = (sbeg_y + sh).min(dbeg_y + dh);
                let right = (sbeg_x + sw).min(dbeg_x + dw);
                for sy in top..bot {
                    for sx in left..right {
                        if let Some(cell) = st.get_cell(src_id, sy - sbeg_y, sx - sbeg_x)
                            && (!skip_spaces || cell.ch != ' ')
                        {
                            st.set_cell(dst_id, sy - dbeg_y, sx - dbeg_x, cell);
                        }
                    }
                }
            });
            Ok(Value::none())
        }
        7 => {
            let sminrow = as_i64_arg(&args[1], "sminrow")?;
            let smincol = as_i64_arg(&args[2], "smincol")?;
            let dminrow = as_i64_arg(&args[3], "dminrow")?;
            let dmincol = as_i64_arg(&args[4], "dmincol")?;
            let dmaxrow = as_i64_arg(&args[5], "dmaxrow")?;
            let dmaxcol = as_i64_arg(&args[6], "dmaxcol")?;
            if sminrow < 0
                || smincol < 0
                || dminrow < 0
                || dmincol < 0
                || dmaxrow < dminrow
                || dmaxcol < dmincol
            {
                return Err(raise_curses_error(interp, "copywin() returned ERR"));
            }
            eng::with_state_mut(|st| {
                let rows = (dmaxrow - dminrow + 1) as usize;
                let cols = (dmaxcol - dmincol + 1) as usize;
                for dy in 0..rows {
                    for dx in 0..cols {
                        if let Some(cell) =
                            st.get_cell(src_id, sminrow as usize + dy, smincol as usize + dx)
                            && (!skip_spaces || cell.ch != ' ')
                        {
                            st.set_cell(dst_id, dminrow as usize + dy, dmincol as usize + dx, cell);
                        }
                    }
                }
            });
            Ok(Value::none())
        }
        _ => Err(PyError::named(
            "TypeError",
            "overlay/overwrite requires 1 or 7 arguments".to_string(),
        )),
    }
}
