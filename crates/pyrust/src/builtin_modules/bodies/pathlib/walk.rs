//! Lazy Python 3.12 `Path.walk()` generator.
//!
//! Mirrors CPython's explicit-stack algorithm: each step scans one directory,
//! and a top-down walk reads the yielded `dirnames` list again on the next
//! step, so callers can prune or reorder it in place. Bottom-up walks queue
//! the `(dirpath, dirnames, filenames)` tuple below the children it covers.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, ProviderIterator};
use crate::value::{PyClass, Value, ValueKind};

use super::class_registry::{
    PathFlavor, expect_self, flavor_for_instance, get_path, make_path_instance_for_class,
};
use super::filesystem::{MethodSig, bind_method_args, method_qualname};
use super::lexical::parse_path;

/// One explicit-stack entry: a directory still to scan, or a bottom-up
/// result waiting for its subtree.
enum WalkItem {
    Dir { value: Value, path: String },
    Ready(Value),
}

/// Boolean options are evaluated lazily, on the first `next()`, like the
/// body of CPython's generator function.
enum WalkFlag {
    Pending(Value),
    Ready(bool),
}

struct WalkState {
    stack: Vec<WalkItem>,
    /// Top-down: the directory just yielded and its (caller-mutable)
    /// `dirnames` list, expanded into children on the next step.
    pending: Option<(String, Value)>,
    top_down: WalkFlag,
    follow_symlinks: WalkFlag,
    on_error: Option<Value>,
    class: Rc<RefCell<PyClass>>,
    flavor: PathFlavor,
    done: bool,
}

/// `Path.walk(top_down=True, on_error=None, follow_symlinks=False)`.
pub(super) fn path_walk(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let bound = bind_method_args(
        args,
        &MethodSig {
            qualname: method_qualname(fn_name),
            positional: &["top_down", "on_error", "follow_symlinks"],
            required: 0,
            keyword_only: &[],
        },
    )?;
    let flag = |slot: Option<&Value>, default: bool| match slot {
        Some(value) => WalkFlag::Pending(value.clone()),
        None => WalkFlag::Ready(default),
    };
    let state = WalkState {
        stack: vec![WalkItem::Dir {
            value: args[0].value.clone(),
            path: get_path(&inst, fn_name)?,
        }],
        pending: None,
        top_down: flag(bound[0], true),
        follow_symlinks: flag(bound[2], false),
        on_error: bound[1].filter(|value| !value.is_none()).cloned(),
        class: Rc::clone(&inst.borrow().class),
        flavor: flavor_for_instance(&inst),
        done: false,
    };
    Ok(Value::generator(Box::new(ProviderIterator::new(
        Box::new(state),
        advance_walk,
        "generator",
        "generator",
        None,
    ))))
}

fn walk_state_error() -> PyError {
    PyError::runtime("internal: Path.walk generator state corrupted".to_string())
}

fn with_state<R>(
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
    f: impl FnOnce(&mut WalkState) -> R,
) -> Result<R> {
    let mut borrow = state_rc.borrow_mut();
    let state = borrow
        .downcast_mut::<WalkState>()
        .ok_or_else(walk_state_error)?;
    Ok(f(state))
}

fn resolve_flag(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
    select: fn(&mut WalkState) -> &mut WalkFlag,
) -> Result<bool> {
    let current = with_state(state_rc, |state| match select(state) {
        WalkFlag::Ready(value) => WalkFlag::Ready(*value),
        WalkFlag::Pending(value) => WalkFlag::Pending(value.clone()),
    })?;
    match current {
        WalkFlag::Ready(value) => Ok(value),
        WalkFlag::Pending(value) => {
            let resolved = interp.truthy_value(&value)?;
            with_state(state_rc, |state| *select(state) = WalkFlag::Ready(resolved))?;
            Ok(resolved)
        }
    }
}

fn advance_walk(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    if with_state(state_rc, |state| state.done)? {
        return Ok(None);
    }
    let result = walk_step(interp, state_rc);
    if !matches!(result, Ok(Some(_))) {
        // A finished or failed generator stays exhausted, as in CPython.
        with_state(state_rc, |state| {
            state.done = true;
            state.stack.clear();
            state.pending = None;
        })?;
    }
    result
}

fn walk_step(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let top_down = resolve_flag(interp, state_rc, |state| &mut state.top_down)?;
    let follow_symlinks = resolve_flag(interp, state_rc, |state| &mut state.follow_symlinks)?;
    loop {
        if let Some((parent, dirnames)) = with_state(state_rc, |state| state.pending.take())? {
            let names = dirname_strings(interp, &dirnames)?;
            with_state(state_rc, |state| push_children(state, &parent, &names))?;
        }
        let Some(item) = with_state(state_rc, |state| state.stack.pop())? else {
            return Ok(None);
        };
        let (value, path) = match item {
            WalkItem::Ready(result) => return Ok(Some(result)),
            WalkItem::Dir { value, path } => (value, path),
        };
        let (dirs, files) = match scan_directory(&path, follow_symlinks) {
            Ok(entries) => entries,
            Err(ScanError::Open(error)) => {
                // Unreadable directories are skipped after reporting, like
                // `os.walk`.
                let on_error = with_state(state_rc, |state| state.on_error.clone())?;
                if let Some(callback) = on_error {
                    let exception = interp.materialize_pyerror(error)?;
                    interp.call_function_expanded(
                        callback,
                        &[ExpandedCallArg {
                            name: None,
                            value: exception,
                        }],
                    )?;
                }
                continue;
            }
            Err(ScanError::Iterate(error)) => return Err(error),
        };
        let dirnames = Value::list(dirs.iter().cloned().map(Value::string).collect());
        let filenames = Value::list(files.into_iter().map(Value::string).collect());
        let result = Value::tuple(vec![value, dirnames.clone(), filenames]);
        if top_down {
            with_state(state_rc, |state| state.pending = Some((path, dirnames)))?;
            return Ok(Some(result));
        }
        with_state(state_rc, |state| {
            state.stack.push(WalkItem::Ready(result));
            push_children(state, &path, &dirs);
        })?;
    }
}

/// Snapshot the (possibly caller-mutated) `dirnames` list; entries are
/// formatted with `str()` as CPython's `f'{path}{sep}{name}'` does.
fn dirname_strings(interp: &mut Interpreter, dirnames: &Value) -> Result<Vec<String>> {
    let items = interp.collect_iterable(dirnames)?;
    let mut names = Vec::with_capacity(items.len());
    for item in &items {
        names.push(match item.kind() {
            ValueKind::Str(name) => name.to_string(),
            _ => interp.render_value_as_str(item)?,
        });
    }
    Ok(names)
}

/// Push `parent / name` for each name so the first name is popped first
/// (`paths += [path._make_child_relpath(d) for d in reversed(dirnames)]`).
fn push_children(state: &mut WalkState, parent: &str, names: &[String]) {
    let sep = match state.flavor {
        PathFlavor::Posix => "/",
        PathFlavor::Windows => "\\",
    };
    let has_tail = !parse_path(parent, state.flavor).tail.is_empty();
    for name in names.iter().rev() {
        // `_make_child_relpath`: join with the separator after a tail, append
        // directly to a bare anchor, and replace an empty (`.`) path.
        let child = if has_tail {
            format!("{parent}{sep}{name}")
        } else if parent != "." {
            format!("{parent}{name}")
        } else {
            name.clone()
        };
        let value = make_path_instance_for_class(Rc::clone(&state.class), &child);
        state.stack.push(WalkItem::Dir { value, path: child });
    }
}

enum ScanError {
    /// `os.scandir()` itself failed: reported to `on_error`, then skipped.
    Open(PyError),
    /// Reading an entry failed mid-iteration: propagates like CPython.
    Iterate(PyError),
}

/// `os.scandir(path)` split into directory and non-directory names using
/// `DirEntry.is_dir(follow_symlinks=...)` semantics.
fn scan_directory(
    path: &str,
    follow_symlinks: bool,
) -> std::result::Result<(Vec<String>, Vec<String>), ScanError> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| ScanError::Open(PyError::from_io_error(&error, Some(path))))?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| ScanError::Iterate(PyError::from_io_error(&error, Some(path))))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry_is_dir(&entry, follow_symlinks) {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    Ok((dirs, files))
}

/// `DirEntry.is_dir(follow_symlinks=...)`, with `OSError` treated as `False`.
fn entry_is_dir(entry: &std::fs::DirEntry, follow_symlinks: bool) -> bool {
    let Ok(file_type) = entry.file_type() else {
        return false;
    };
    #[cfg(windows)]
    {
        // CPython's Windows `DirEntry` only treats `IO_REPARSE_TAG_SYMLINK`
        // as a link; a junction keeps its directory attribute.
        use std::os::windows::fs::FileTypeExt;
        if file_type.is_symlink_dir()
            && crate::builtin_modules::ntpath::ops::path_is_junction(&entry.path())
        {
            return true;
        }
    }
    if file_type.is_symlink() {
        return follow_symlinks && std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
    }
    file_type.is_dir()
}
