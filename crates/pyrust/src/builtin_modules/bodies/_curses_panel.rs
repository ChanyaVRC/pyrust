// `_curses_panel` module — 100% native Rust implementation of Python 3.12's
// curses panel stack extension (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.
// Reference: <https://docs.python.org/3.12/library/curses.panel.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::curses_engine::{self as eng, PanelEntry};
use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

use super::_curses::extract_window_id;

pub(crate) struct PanelHandle {
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

fn raise_panel_error(interp: &mut Interpreter, msg: impl Into<String>) -> PyError {
    let msg_str = msg.into();
    if let Ok(m) = interp.load_module("_curses_panel")
        && let Ok(cls) = interp.get_attr(&m, "error")
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(&msg_str)])
    {
        return PyError::raised(inst);
    }
    PyError::named("error", msg_str)
}

fn make_panel_value(interp: &mut Interpreter, panel_id: usize) -> Result<Value> {
    let mod_val = interp.load_module("_curses_panel")?;
    let cls_val = interp.get_attr(&mod_val, "panel")?;
    let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
        return Err(PyError::runtime(
            "_curses_panel.panel is not a class".to_string(),
        ));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::default(),
    })));
    set_native_state(&inst, PanelHandle { id: panel_id })?;
    Ok(inst)
}

pyrust_module! {
    constants {
        "version" => Value::bytes(b"2.1".to_vec()),
        "__version__" => Value::bytes(b"2.1".to_vec()),
    }

    /// Exception raised when a curses panel operation fails.
    exception error;

    /// Curses panel object representing a window in the panel stack.
    #[native_state(PanelHandle)]
    class panel {
        fn above(#[state] h: &PanelHandle) -> Result<Value> {
            let res = eng::with_state(|st| {
                let idx = st.panels.iter().position(|p| p.id == h.id)?;
                if st.panels[idx].hidden {
                    return None;
                }
                st.panels[(idx + 1)..]
                    .iter()
                    .find(|p| !p.hidden)
                    .map(|p| p.panel_val.clone())
            });
            Ok(res.unwrap_or_else(Value::none))
        }

        fn below(#[state] h: &PanelHandle) -> Result<Value> {
            let res = eng::with_state(|st| {
                let idx = st.panels.iter().position(|p| p.id == h.id)?;
                if st.panels[idx].hidden {
                    return None;
                }
                st.panels[..idx]
                    .iter()
                    .rev()
                    .find(|p| !p.hidden)
                    .map(|p| p.panel_val.clone())
            });
            Ok(res.unwrap_or_else(Value::none))
        }

        fn bottom(#[state] h: &PanelHandle) -> Result<Value> {
            let ok = eng::with_state_mut(|st| {
                let Some(idx) = st.panels.iter().position(|p| p.id == h.id) else {
                    return false;
                };
                let mut entry = st.panels.remove(idx);
                entry.hidden = false;
                st.panels.insert(0, entry);
                true
            });
            if !ok {
                return Err(raise_panel_error(_interp, "bottom_panel() returned ERR"));
            }
            Ok(Value::none())
        }

        fn top(#[state] h: &PanelHandle) -> Result<Value> {
            let ok = eng::with_state_mut(|st| {
                let Some(idx) = st.panels.iter().position(|p| p.id == h.id) else {
                    return false;
                };
                let mut entry = st.panels.remove(idx);
                entry.hidden = false;
                st.panels.push(entry);
                true
            });
            if !ok {
                return Err(raise_panel_error(_interp, "top_panel() returned ERR"));
            }
            Ok(Value::none())
        }

        fn hidden(#[state] h: &PanelHandle) -> Result<Value> {
            let is_hidden = eng::with_state(|st| {
                st.panels
                    .iter()
                    .find(|p| p.id == h.id)
                    .is_some_and(|p| p.hidden)
            });
            Ok(Value::bool_(is_hidden))
        }

        fn hide(#[state] h: &PanelHandle) -> Result<Value> {
            let ok = eng::with_state_mut(|st| {
                let Some(entry) = st.panels.iter_mut().find(|p| p.id == h.id) else {
                    return false;
                };
                entry.hidden = true;
                true
            });
            if !ok {
                return Err(raise_panel_error(_interp, "hide_panel() returned ERR"));
            }
            Ok(Value::none())
        }

        fn show(#[state] h: &PanelHandle) -> Result<Value> {
            let ok = eng::with_state_mut(|st| {
                let Some(idx) = st.panels.iter().position(|p| p.id == h.id) else {
                    return false;
                };
                let mut entry = st.panels.remove(idx);
                entry.hidden = false;
                st.panels.push(entry);
                true
            });
            if !ok {
                return Err(raise_panel_error(_interp, "show_panel() returned ERR"));
            }
            Ok(Value::none())
        }

        #[py_name = "move"]
        fn py_move(#[state] h: &PanelHandle, y: PyIndex, x: PyIndex) -> Result<Value> {
            let yv = y.expect_i64(FN_NAME, "y")?;
            let xv = x.expect_i64(FN_NAME, "x")?;
            let ok = eng::with_state_mut(|st| {
                let Some(win_id) = st.panels.iter().find(|p| p.id == h.id).map(|p| p.win_id)
                else {
                    return false;
                };
                let (lines, cols) = (st.lines, st.cols);
                if let Some(w) = st.windows.get_mut(&win_id)
                    && yv >= 0
                    && xv >= 0
                    && (yv as usize) + w.height <= lines
                    && (xv as usize) + w.width <= cols
                {
                    w.beg_y = yv as usize;
                    w.beg_x = xv as usize;
                    w.touched.fill(true);
                    return true;
                }
                false
            });
            if !ok {
                return Err(raise_panel_error(_interp, "move_panel() returned ERR"));
            }
            Ok(Value::none())
        }

        fn replace(#[state] h: &PanelHandle, win: PyValue) -> Result<Value> {
            let new_win_id = extract_window_id(&win.0)?;
            let ok = eng::with_state_mut(|st| {
                let Some(entry) = st.panels.iter_mut().find(|p| p.id == h.id) else {
                    return false;
                };
                entry.win_id = new_win_id;
                entry.win_val = win.0;
                true
            });
            if !ok {
                return Err(raise_panel_error(_interp, "replace_panel() returned ERR"));
            }
            Ok(Value::none())
        }

        fn set_userptr(#[state] h: &PanelHandle, obj: PyValue) -> Result<Value> {
            let ok = eng::with_state_mut(|st| {
                let Some(entry) = st.panels.iter_mut().find(|p| p.id == h.id) else {
                    return false;
                };
                entry.userptr = Some(obj.0);
                true
            });
            if !ok {
                return Err(raise_panel_error(_interp, "set_panel_userptr() returned ERR"));
            }
            Ok(Value::none())
        }

        fn userptr(#[state] h: &PanelHandle) -> Result<Value> {
            let ptr = eng::with_state(|st| {
                st.panels
                    .iter()
                    .find(|p| p.id == h.id)
                    .and_then(|p| p.userptr.clone())
            });
            match ptr {
                Some(v) => Ok(v),
                None => Err(raise_panel_error(_interp, "userptr() returned NULL")),
            }
        }

        fn window(#[state] h: &PanelHandle) -> Result<Value> {
            let win = eng::with_state(|st| {
                st.panels
                    .iter()
                    .find(|p| p.id == h.id)
                    .map(|p| p.win_val.clone())
            });
            match win {
                Some(v) => Ok(v),
                None => Err(raise_panel_error(_interp, "panel_window() returned NULL")),
            }
        }
    }

    /// Return a panel object, associating it with the given window `win`.
    fn new_panel(#[positional_only] win: PyValue) -> Result<Value> {
        let win_id = extract_window_id(&win.0)?;
        let panel_id = eng::with_state_mut(|st| {
            let id = st.next_panel_id;
            st.next_panel_id += 1;
            id
        });
        let panel_val = make_panel_value(_interp, panel_id)?;
        let entry = PanelEntry {
            id: panel_id,
            win_id,
            win_val: win.0,
            panel_val: panel_val.clone(),
            hidden: false,
            userptr: None,
        };
        eng::with_state_mut(|st| {
            st.panels.push(entry);
        });
        Ok(panel_val)
    }

    /// Return the top panel in the panel stack, or None if the stack is empty.
    fn top_panel() -> Result<Value> {
        let res = eng::with_state(|st| {
            st.panels
                .iter()
                .rev()
                .find(|p| !p.hidden)
                .map(|p| p.panel_val.clone())
        });
        Ok(res.unwrap_or_else(Value::none))
    }

    /// Return the bottom panel in the panel stack, or None if the stack is empty.
    fn bottom_panel() -> Result<Value> {
        let res = eng::with_state(|st| {
            st.panels
                .iter()
                .find(|p| !p.hidden)
                .map(|p| p.panel_val.clone())
        });
        Ok(res.unwrap_or_else(Value::none))
    }

    /// Updates the virtual screen after changes in the panel stack.
    fn update_panels() -> Result<Value> {
        eng::with_state_mut(|st| st.update_panels());
        Ok(Value::none())
    }
}
