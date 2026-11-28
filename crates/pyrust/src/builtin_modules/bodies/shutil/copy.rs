//! File and tree copying of CPython 3.12.3 `Lib/shutil.py`: `copyfileobj`,
//! `_samefile`, `_stat`, `_islink`, `copyfile`, `copymode`, `_copyxattr`,
//! `copystat`, `copy`, `copy2`, `ignore_patterns`, `_copytree` and
//! `copytree`.
//!
//! Every file-system step goes through the `os` / `os.path` functions the
//! source calls, so their error objects (`errno`, `filename`, ...) are the
//! ones the runtime's `os` raises.  `copy2` takes the source's portable path
//! (`copyfile` + `copystat`): the runtime has no `_winapi.CopyFile2`.

use std::rc::Rc;

use crate::builtin_modules::json::args::{self, Sig};
use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value};

use super::support::{
    self, Caught, S_IFIFO, audit, builtin, builtin_exc, call, call_kw, collect, contains, equal,
    getattr_opt, getitem, global, global_class, hasattr, is_direntry, is_same, method, method_kw,
    os_attr, os_call, os_call_kw, os_module, os_name_is_nt, os_path, path_call, path_test, percent,
    raise_from, raise_global, repr_of, s_imode, s_isfmt, sig, str_of, truthy, try_except, try_os,
};

/// `COPY_BUFSIZE` of the source.
pub(super) const COPY_BUFSIZE: i64 = if cfg!(windows) {
    1024 * 1024
} else {
    64 * 1024
};

const KW_FOLLOW: &[&str] = &["follow_symlinks"];

const SIG_COPYFILEOBJ: Sig = sig("copyfileobj", &["fsrc", "fdst", "length"], 1, &[]);
const SIG_SAMEFILE: Sig = sig("_samefile", &["src", "dst"], 0, &[]);
const SIG_STAT: Sig = sig("_stat", &["fn"], 0, &[]);
const SIG_ISLINK: Sig = sig("_islink", &["fn"], 0, &[]);
const SIG_COPYFILE: Sig = sig("copyfile", &["src", "dst"], 0, KW_FOLLOW);
const SIG_COPYMODE: Sig = sig("copymode", &["src", "dst"], 0, KW_FOLLOW);
const SIG_COPYXATTR: Sig = sig("_copyxattr", &["src", "dst"], 0, KW_FOLLOW);
const SIG_COPYSTAT: Sig = sig("copystat", &["src", "dst"], 0, KW_FOLLOW);
const SIG_COPY: Sig = sig("copy", &["src", "dst"], 0, KW_FOLLOW);
const SIG_COPY2: Sig = sig("copy2", &["src", "dst"], 0, KW_FOLLOW);
const SIG_IGNORE_CALL: Sig = sig(
    "ignore_patterns.<locals>._ignore_patterns",
    &["path", "names"],
    0,
    &[],
);
const TREE_PARAMS: &[&str] = &[
    "src",
    "dst",
    "symlinks",
    "ignore",
    "copy_function",
    "ignore_dangling_symlinks",
    "dirs_exist_ok",
];
const SIG_COPYTREE: Sig = sig("copytree", TREE_PARAMS, 5, &[]);
const SIG_COPYTREE_ENTRIES: Sig = sig(
    "_copytree",
    &[
        "entries",
        "src",
        "dst",
        "symlinks",
        "ignore",
        "copy_function",
        "ignore_dangling_symlinks",
        "dirs_exist_ok",
    ],
    1,
    &[],
);

/// The `follow_symlinks` keyword (default `True`).
fn follow_arg(interp: &mut Interpreter, b: &mut args::Bound, slot: usize) -> Result<bool> {
    match b.take(slot) {
        Some(value) => truthy(interp, &value),
        None => Ok(true),
    }
}

fn bind2(sig: &Sig, args: &[ExpandedCallArg]) -> Result<(Value, Value, args::Bound)> {
    let mut b = args::bind(sig, args)?;
    let first = b.take_or_none(0);
    let second = b.take_or_none(1);
    Ok((first, second, b))
}

// ---------------------------------------------------------------------------
// copyfileobj
// ---------------------------------------------------------------------------

fn copy_bufsize() -> Value {
    global("COPY_BUFSIZE").unwrap_or_else(|| Value::int(COPY_BUFSIZE))
}

/// `copyfileobj(fsrc, fdst, length=0)`.
pub(super) fn copyfileobj(
    interp: &mut Interpreter,
    fsrc: &Value,
    fdst: &Value,
    length: Value,
) -> Result<()> {
    let length = if truthy(interp, &length)? {
        length
    } else {
        copy_bufsize()
    };
    let read = interp.get_attr(fsrc, "read")?;
    let write = interp.get_attr(fdst, "write")?;
    loop {
        let buf = call(interp, read.clone(), std::slice::from_ref(&length))?;
        if !truthy(interp, &buf)? {
            return Ok(());
        }
        call(interp, write.clone(), &[buf])?;
    }
}

pub(super) fn copyfileobj_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_COPYFILEOBJ, args)?;
    let fsrc = b.take_or_none(0);
    let fdst = b.take_or_none(1);
    let length = b.take_or(2, || Value::int(0));
    copyfileobj(interp, &fsrc, &fdst, length)?;
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// _samefile / _stat / _islink
// ---------------------------------------------------------------------------

/// `_samefile(src, dst)`.
pub(super) fn samefile(interp: &mut Interpreter, src: &Value, dst: &Value) -> Result<bool> {
    let os_path = os_path(interp)?;
    // Macintosh, Unix.
    if is_direntry(interp, src)? && hasattr(interp, &os_path, "samestat")? {
        let same = try_os(interp, |interp| {
            let a = method(interp, src, "stat", &[])?;
            let b = os_call(interp, "stat", std::slice::from_ref(dst))?;
            let result = method(interp, &os_path, "samestat", &[a, b])?;
            truthy(interp, &result)
        })?;
        return Ok(same.unwrap_or(false));
    }
    if hasattr(interp, &os_path, "samefile")? {
        let same = try_os(interp, |interp| {
            let result = method(interp, &os_path, "samefile", &[src.clone(), dst.clone()])?;
            truthy(interp, &result)
        })?;
        return Ok(same.unwrap_or(false));
    }
    // All other platforms: check for same pathname.
    let mut normalized = Vec::with_capacity(2);
    for path in [src, dst] {
        let absolute = method(interp, &os_path, "abspath", std::slice::from_ref(path))?;
        normalized.push(method(interp, &os_path, "normcase", &[absolute])?);
    }
    equal(interp, &normalized[0], &normalized[1])
}

pub(super) fn samefile_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, _) = bind2(&SIG_SAMEFILE, args)?;
    Ok(Value::bool_(samefile(interp, &src, &dst)?))
}

/// `_stat(fn)`.
pub(super) fn stat_of(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    if is_direntry(interp, path)? {
        method(interp, path, "stat", &[])
    } else {
        os_call(interp, "stat", std::slice::from_ref(path))
    }
}

pub(super) fn stat_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_STAT, args)?;
    let path = b.take_or_none(0);
    stat_of(interp, &path)
}

/// `_islink(fn)`.
pub(super) fn islink(interp: &mut Interpreter, path: &Value) -> Result<bool> {
    let result = if is_direntry(interp, path)? {
        method(interp, path, "is_symlink", &[])?
    } else {
        path_call(interp, "islink", std::slice::from_ref(path))?
    };
    truthy(interp, &result)
}

pub(super) fn islink_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_ISLINK, args)?;
    let path = b.take_or_none(0);
    Ok(Value::bool_(islink(interp, &path)?))
}

// ---------------------------------------------------------------------------
// copyfile
// ---------------------------------------------------------------------------

/// `copyfile(src, dst, *, follow_symlinks=True)`.
pub(super) fn copyfile(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    follow_symlinks: bool,
) -> Result<Value> {
    audit(interp, "shutil.copyfile", &[src.clone(), dst.clone()])?;

    if samefile(interp, src, dst)? {
        let msg = format!(
            "{} and {} are the same file",
            repr_of(interp, src)?,
            repr_of(interp, dst)?
        );
        return Err(raise_global(interp, "SameFileError", &[Value::string(msg)]));
    }

    for path in [src, dst] {
        let Ok(st) = try_os(interp, |interp| stat_of(interp, path))? else {
            // File most likely does not exist.
            continue;
        };
        let mode = interp.get_attr(&st, "st_mode")?;
        if s_isfmt(interp, &mode, S_IFIFO)? {
            let shown = if is_direntry(interp, path)? {
                interp.get_attr(path, "path")?
            } else {
                path.clone()
            };
            let msg = percent(interp, "`%s` is a named pipe", vec![shown])?;
            return Err(raise_global(interp, "SpecialFileError", &[msg]));
        }
    }

    if !follow_symlinks && islink(interp, src)? {
        let target = os_call(interp, "readlink", std::slice::from_ref(src))?;
        os_call(interp, "symlink", &[target, dst.clone()])?;
        return Ok(dst.clone());
    }

    let fsrc = builtin(interp, "open", &[src.clone(), Value::string("rb")])?;
    let copied = copy_to_destination(interp, &fsrc, dst);
    let closed = method(interp, &fsrc, "close", &[]);
    copied?;
    closed?;
    Ok(dst.clone())
}

/// The `try: with open(dst, 'wb') as fdst: ... except IsADirectoryError`
/// part of `copyfile`.
fn copy_to_destination(interp: &mut Interpreter, fsrc: &Value, dst: &Value) -> Result<()> {
    let is_a_directory = builtin_exc("IsADirectoryError")?;
    let outcome = try_except(interp, &[is_a_directory], |interp| {
        let fdst = builtin(interp, "open", &[dst.clone(), Value::string("wb")])?;
        let copied = copyfileobj(interp, fsrc, &fdst, Value::int(0));
        let closed = method(interp, &fdst, "close", &[]);
        copied?;
        closed?;
        Ok(())
    })?;
    let Err(caught) = outcome else {
        return Ok(());
    };
    // Issue 43219, raise a less confusing exception.
    if path_test(interp, "exists", std::slice::from_ref(dst))? {
        return Err(caught.error);
    }
    let shown = str_of(interp, dst)?;
    let msg = format!(
        "Directory does not exist: {}",
        shown.as_str().unwrap_or_default()
    );
    let exc = support::new_builtin_exc(interp, "FileNotFoundError", &[Value::string(msg)])?;
    Err(raise_from(interp, exc, caught.exc))
}

pub(super) fn copyfile_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPYFILE, args)?;
    let follow = follow_arg(interp, &mut b, 2)?;
    copyfile(interp, &src, &dst, follow)
}

// ---------------------------------------------------------------------------
// copymode / copystat
// ---------------------------------------------------------------------------

/// `os.path.realpath(dst, strict=True)`.
fn realpath_strict(interp: &mut Interpreter, dst: &Value) -> Result<Value> {
    let realpath = support::path_attr(interp, "realpath")?;
    call_kw(
        interp,
        realpath,
        std::slice::from_ref(dst),
        &[("strict", Value::bool_(true))],
    )
}

/// `copymode(src, dst, *, follow_symlinks=True)`.
pub(super) fn copymode(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    follow_symlinks: bool,
) -> Result<()> {
    audit(interp, "shutil.copymode", &[src.clone(), dst.clone()])?;

    let nt = os_name_is_nt(interp)?;
    let mut dst = dst.clone();
    let both_links = !follow_symlinks
        && islink(interp, src)?
        && path_test(interp, "islink", std::slice::from_ref(&dst))?;
    let (st, chmod) = if both_links {
        let chmod = if nt {
            os_attr(interp, "chmod")?
        } else {
            let os = os_module(interp)?;
            match getattr_opt(interp, &os, "lchmod")? {
                Some(lchmod) => lchmod,
                None => return Ok(()),
            }
        };
        (os_call(interp, "lstat", std::slice::from_ref(src))?, chmod)
    } else {
        if nt && path_test(interp, "islink", std::slice::from_ref(&dst))? {
            dst = realpath_strict(interp, &dst)?;
        }
        let chmod = os_attr(interp, "chmod")?;
        (stat_of(interp, src)?, chmod)
    };
    let mode = s_imode(interp, &st)?;
    call(interp, chmod, &[dst, mode])?;
    Ok(())
}

pub(super) fn copymode_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPYMODE, args)?;
    let follow = follow_arg(interp, &mut b, 2)?;
    copymode(interp, &src, &dst, follow)?;
    Ok(Value::none())
}

/// `errno` values `_copyxattr` tolerates.
fn errno_in(interp: &mut Interpreter, exc: &Value, names: &[&str]) -> Result<bool> {
    let errno = interp.get_attr(exc, "errno")?;
    let errno_module = support::dep(interp, "errno")?;
    for name in names {
        if let Some(code) = getattr_opt(interp, &errno_module, name)?
            && equal(interp, &errno, &code)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `_copyxattr(src, dst, *, follow_symlinks=True)`: copies the extended
/// attributes where the runtime's `os` provides `listxattr`; a no-op
/// otherwise (as the source's fallback definition).
pub(super) fn copyxattr(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    follow_symlinks: bool,
) -> Result<()> {
    let os = os_module(interp)?;
    let Some(listxattr) = getattr_opt(interp, &os, "listxattr")? else {
        return Ok(());
    };
    let follow = Value::bool_(follow_symlinks);
    let listed = try_os(interp, |interp| {
        call_kw(
            interp,
            listxattr,
            std::slice::from_ref(src),
            &[("follow_symlinks", follow.clone())],
        )
    })?;
    let names = match listed {
        Ok(names) => names,
        Err(caught) => {
            if errno_in(interp, &caught.exc, &["ENOTSUP", "ENODATA", "EINVAL"])? {
                return Ok(());
            }
            return Err(caught.error);
        }
    };
    for name in collect(interp, &names)? {
        let copied = try_os(interp, |interp| {
            let value = os_call_kw(
                interp,
                "getxattr",
                &[src.clone(), name.clone()],
                &[("follow_symlinks", follow.clone())],
            )?;
            os_call_kw(
                interp,
                "setxattr",
                &[dst.clone(), name.clone(), value],
                &[("follow_symlinks", follow.clone())],
            )
        })?;
        if let Err(caught) = copied
            && !errno_in(
                interp,
                &caught.exc,
                &["EPERM", "ENOTSUP", "ENODATA", "EINVAL", "EACCES"],
            )?
        {
            return Err(caught.error);
        }
    }
    Ok(())
}

pub(super) fn copyxattr_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPYXATTR, args)?;
    let follow = follow_arg(interp, &mut b, 2)?;
    copyxattr(interp, &src, &dst, follow)?;
    Ok(Value::none())
}

/// `lookup(name)` of `copystat`: the `os` function, or `None` for the
/// source's `_nop` (missing, or not in `os.supports_follow_symlinks` when
/// symlinks are not followed).
fn copystat_lookup(interp: &mut Interpreter, name: &str, follow: bool) -> Result<Option<Value>> {
    let os = os_module(interp)?;
    let Some(function) = getattr_opt(interp, &os, name)? else {
        return Ok(None);
    };
    if follow {
        return Ok(Some(function));
    }
    let supported = interp.get_attr(&os, "supports_follow_symlinks")?;
    if contains(interp, &supported, &function)? {
        Ok(Some(function))
    } else {
        Ok(None)
    }
}

/// `copystat(src, dst, *, follow_symlinks=True)`.
pub(super) fn copystat(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    follow_symlinks: bool,
) -> Result<()> {
    audit(interp, "shutil.copystat", &[src.clone(), dst.clone()])?;

    // follow symlinks (aka don't not follow symlinks)
    let follow = follow_symlinks
        || !(islink(interp, src)? && path_test(interp, "islink", std::slice::from_ref(dst))?);
    let follow_kw = [("follow_symlinks", Value::bool_(follow))];

    let st = if is_direntry(interp, src)? {
        method_kw(interp, src, "stat", &[], &follow_kw)?
    } else {
        match copystat_lookup(interp, "stat", follow)? {
            Some(stat) => call_kw(interp, stat, std::slice::from_ref(src), &follow_kw)?,
            None => Value::none(),
        }
    };
    let mode = s_imode(interp, &st)?;
    if let Some(utime) = copystat_lookup(interp, "utime", follow)? {
        let atime = interp.get_attr(&st, "st_atime_ns")?;
        let mtime = interp.get_attr(&st, "st_mtime_ns")?;
        call_kw(
            interp,
            utime,
            std::slice::from_ref(dst),
            &[
                ("ns", Value::tuple(vec![atime, mtime])),
                ("follow_symlinks", Value::bool_(follow)),
            ],
        )?;
    }
    // We must copy extended attributes before the file is (potentially)
    // chmod()'ed read-only, otherwise setxattr() will error with -EACCES.
    copyxattr(interp, src, dst, follow)?;

    let mut dst = dst.clone();
    let chmod_result = if os_name_is_nt(interp)? {
        if follow {
            if path_test(interp, "islink", std::slice::from_ref(&dst))? {
                dst = realpath_strict(interp, &dst)?;
            }
            match copystat_lookup(interp, "chmod", follow)? {
                Some(chmod) => {
                    let pos = [dst.clone(), mode.clone()];
                    try_chmod(interp, |interp| call_kw(interp, chmod, &pos, &follow_kw))
                }
                None => Ok(()),
            }
        } else {
            // `def _chmod(*args, **kwargs): os.chmod(*args)`.
            let pos = [dst.clone(), mode.clone()];
            try_chmod(interp, |interp| os_call(interp, "chmod", &pos))
        }
    } else {
        match copystat_lookup(interp, "chmod", follow)? {
            Some(chmod) => {
                let pos = [dst.clone(), mode.clone()];
                try_chmod(interp, |interp| call_kw(interp, chmod, &pos, &follow_kw))
            }
            None => Ok(()),
        }
    };
    chmod_result?;

    if let Some(flags) = getattr_opt(interp, &st, "st_flags")? {
        let Some(chflags) = copystat_lookup(interp, "chflags", follow)? else {
            return Ok(());
        };
        let changed = try_os(interp, |interp| {
            call_kw(interp, chflags, &[dst.clone(), flags], &follow_kw)
        })?;
        if let Err(caught) = changed
            && !errno_in(interp, &caught.exc, &["EOPNOTSUPP", "ENOTSUP"])?
        {
            return Err(caught.error);
        }
    }
    Ok(())
}

/// `try: _chmod(...) except NotImplementedError: pass`.
fn try_chmod(
    interp: &mut Interpreter,
    f: impl FnOnce(&mut Interpreter) -> Result<Value>,
) -> Result<()> {
    let not_implemented = builtin_exc("NotImplementedError")?;
    try_except(interp, &[not_implemented], f)?.ok();
    Ok(())
}

pub(super) fn copystat_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPYSTAT, args)?;
    let follow = follow_arg(interp, &mut b, 2)?;
    copystat(interp, &src, &dst, follow)?;
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// copy / copy2
// ---------------------------------------------------------------------------

/// `if os.path.isdir(dst): dst = os.path.join(dst, os.path.basename(src))`.
fn destination_in_dir(interp: &mut Interpreter, src: &Value, dst: &Value) -> Result<Value> {
    if path_test(interp, "isdir", std::slice::from_ref(dst))? {
        let name = path_call(interp, "basename", std::slice::from_ref(src))?;
        return path_call(interp, "join", &[dst.clone(), name]);
    }
    Ok(dst.clone())
}

/// `copy(src, dst, *, follow_symlinks=True)`.
pub(super) fn copy(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    follow_symlinks: bool,
) -> Result<Value> {
    let dst = destination_in_dir(interp, src, dst)?;
    copyfile(interp, src, &dst, follow_symlinks)?;
    copymode(interp, src, &dst, follow_symlinks)?;
    Ok(dst)
}

pub(super) fn copy_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPY, args)?;
    let follow = follow_arg(interp, &mut b, 2)?;
    copy(interp, &src, &dst, follow)
}

/// `copy2(src, dst, *, follow_symlinks=True)`.
pub(super) fn copy2(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    follow_symlinks: bool,
) -> Result<Value> {
    let dst = destination_in_dir(interp, src, dst)?;
    copyfile(interp, src, &dst, follow_symlinks)?;
    copystat(interp, src, &dst, follow_symlinks)?;
    Ok(dst)
}

pub(super) fn copy2_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPY2, args)?;
    let follow = follow_arg(interp, &mut b, 2)?;
    copy2(interp, &src, &dst, follow)
}

// ---------------------------------------------------------------------------
// ignore_patterns
// ---------------------------------------------------------------------------

/// The patterns captured by the `_ignore_patterns` closure.
pub(super) struct IgnorePatternsState {
    patterns: Vec<Value>,
}

pub(super) const IGNORE_PATTERNS_TYPE: &str = "_ignore_patterns";

/// `ignore_patterns(*patterns)`.
pub(super) fn ignore_patterns_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    if let Some(name) = args.iter().find_map(|arg| arg.name.as_ref()) {
        return Err(args::type_error(format!(
            "ignore_patterns() got an unexpected keyword argument '{name}'"
        )));
    }
    let patterns = args.iter().map(|arg| arg.value.clone()).collect();
    let class = support::ignore_class()?;
    let closure = Value::py_instance(Rc::new(std::cell::RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    crate::builtin_modules::macros::native_state::set_native_state(
        &closure,
        IgnorePatternsState { patterns },
    )?;
    // The names of the source's closure function.
    interp.assign_attr(
        closure.clone(),
        "__name__",
        Value::string("_ignore_patterns"),
    )?;
    interp.assign_attr(
        closure.clone(),
        "__qualname__",
        Value::string("ignore_patterns.<locals>._ignore_patterns"),
    )?;
    Ok(closure)
}

/// `ignore_patterns.<locals>._ignore_patterns(path, names)`.
pub(super) fn ignore_patterns_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, rest) = args.split_first().ok_or_else(|| {
        args::type_error("_ignore_patterns() missing 2 required positional arguments")
    })?;
    let patterns = crate::builtin_modules::macros::native_state::with_native_state(
        &this.value,
        IGNORE_PATTERNS_TYPE,
        |state: &IgnorePatternsState| Ok(state.patterns.clone()),
    )?;
    let mut b = args::bind(&SIG_IGNORE_CALL, rest)?;
    let _path = b.take_or_none(0);
    let names = b.take_or_none(1);
    let fnmatch = support::dep(interp, "fnmatch")?;
    let filter = interp.get_attr(&fnmatch, "filter")?;
    let mut ignored = Vec::new();
    for pattern in patterns {
        let matched = call(interp, filter.clone(), &[names.clone(), pattern])?;
        ignored.extend(collect(interp, &matched)?);
    }
    builtin(interp, "set", &[Value::list(ignored)])
}

/// `repr()` of the closure, in the shape of a function's repr.
pub(super) fn ignore_patterns_repr(args: &[ExpandedCallArg]) -> Result<Value> {
    let id = args
        .first()
        .and_then(|arg| arg.value.object_id().as_int())
        .unwrap_or(0);
    Ok(Value::string(format!(
        "<function ignore_patterns.<locals>._ignore_patterns at {id:#x}>"
    )))
}

// ---------------------------------------------------------------------------
// copytree
// ---------------------------------------------------------------------------

/// The pass-through arguments of `copytree` / `_copytree`.
struct TreeArgs {
    symlinks: Value,
    ignore: Value,
    copy_function: Value,
    ignore_dangling_symlinks: Value,
    dirs_exist_ok: Value,
}

fn default_copy2() -> Value {
    global("copy2").unwrap_or_else(|| Value::builtin_function("shutil.copy2"))
}

fn tree_args(b: &mut args::Bound, first: usize) -> TreeArgs {
    TreeArgs {
        symlinks: b.take_or(first, || Value::bool_(false)),
        ignore: b.take_or_none(first + 1),
        copy_function: b.take_or(first + 2, default_copy2),
        ignore_dangling_symlinks: b.take_or(first + 3, || Value::bool_(false)),
        dirs_exist_ok: b.take_or(first + 4, || Value::bool_(false)),
    }
}

/// `with os.scandir(src) as itr: entries = list(itr)`.
pub(super) fn scandir_list(interp: &mut Interpreter, path: &Value) -> Result<Vec<Value>> {
    let iterator = os_call(interp, "scandir", std::slice::from_ref(path))?;
    let entries = collect(interp, &iterator);
    let closed = method(interp, &iterator, "close", &[]);
    let entries = entries?;
    closed?;
    Ok(entries)
}

/// `copytree(src, dst, symlinks=False, ignore=None, copy_function=copy2,
/// ignore_dangling_symlinks=False, dirs_exist_ok=False)`.
fn copytree(interp: &mut Interpreter, src: &Value, dst: &Value, tree: &TreeArgs) -> Result<Value> {
    audit(interp, "shutil.copytree", &[src.clone(), dst.clone()])?;
    let entries = scandir_list(interp, src)?;
    copytree_entries(interp, entries, src, dst, tree)
}

pub(super) fn copytree_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (src, dst, mut b) = bind2(&SIG_COPYTREE, args)?;
    let tree = tree_args(&mut b, 2);
    copytree(interp, &src, &dst, &tree)
}

/// `copytree(src, real_dst, copy_function=copy_function, symlinks=True)` as
/// `move` calls it.
pub(super) fn copytree_for_move(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    copy_function: Value,
) -> Result<Value> {
    let tree = TreeArgs {
        symlinks: Value::bool_(true),
        ignore: Value::none(),
        copy_function,
        ignore_dangling_symlinks: Value::bool_(false),
        dirs_exist_ok: Value::bool_(false),
    };
    copytree(interp, src, dst, &tree)
}

pub(super) fn copytree_entries_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_COPYTREE_ENTRIES, args)?;
    let entries = b.take_or_none(0);
    let src = b.take_or_none(1);
    let dst = b.take_or_none(2);
    let tree = TreeArgs {
        symlinks: b.take_or_none(3),
        ignore: b.take_or_none(4),
        copy_function: b.take_or_none(5),
        ignore_dangling_symlinks: b.take_or_none(6),
        dirs_exist_ok: b.take_or(7, || Value::bool_(false)),
    };
    let entries = collect(interp, &entries)?;
    copytree_entries(interp, entries, &src, &dst, &tree)
}

/// `_copytree(entries, src, dst, symlinks, ignore, copy_function,
/// ignore_dangling_symlinks, dirs_exist_ok=False)`.
fn copytree_entries(
    interp: &mut Interpreter,
    entries: Vec<Value>,
    src: &Value,
    dst: &Value,
    tree: &TreeArgs,
) -> Result<Value> {
    let ignored_names = if tree.ignore.is_none() {
        None
    } else {
        let mut names = Vec::with_capacity(entries.len());
        for entry in &entries {
            names.push(interp.get_attr(entry, "name")?);
        }
        let src_path = os_call(interp, "fspath", std::slice::from_ref(src))?;
        Some(call(
            interp,
            tree.ignore.clone(),
            &[src_path, Value::list(names)],
        )?)
    };

    os_call_kw(
        interp,
        "makedirs",
        std::slice::from_ref(dst),
        &[("exist_ok", tree.dirs_exist_ok.clone())],
    )?;
    let mut errors = Vec::new();
    let use_srcentry = global("copy2").is_some_and(|f| is_same(&tree.copy_function, &f))
        || global("copy").is_some_and(|f| is_same(&tree.copy_function, &f));
    let error_class = global_class("Error")?;
    let os_error = builtin_exc("OSError")?;
    let nt = os_name_is_nt(interp)?;

    for entry in entries {
        let name = interp.get_attr(&entry, "name")?;
        if let Some(ignored) = &ignored_names
            && contains(interp, ignored, &name)?
        {
            continue;
        }
        let srcname = path_call(interp, "join", &[src.clone(), name.clone()])?;
        let dstname = path_call(interp, "join", &[dst.clone(), name])?;
        let srcobj = if use_srcentry {
            entry.clone()
        } else {
            srcname.clone()
        };
        let outcome = try_except(interp, &[error_class.clone(), os_error.clone()], |interp| {
            copy_entry_into(interp, &entry, &srcname, &dstname, &srcobj, tree, nt)
        })?;
        match outcome {
            Ok(()) => {}
            // catch the Error from the recursive copytree so that we can
            // continue with other files
            Err(Caught { exc, index: 0, .. }) => {
                let exc_args = interp.get_attr(&exc, "args")?;
                let first = getitem(interp, &exc_args, Value::int(0))?;
                errors.extend(collect(interp, &first)?);
            }
            Err(Caught { exc, .. }) => {
                let why = str_of(interp, &exc)?;
                errors.push(Value::tuple(vec![srcname, dstname, why]));
            }
        }
    }

    if let Err(caught) = try_os(interp, |interp| copystat(interp, src, dst, true))? {
        // Copying file access times may fail on Windows
        let winerror = getattr_opt(interp, &caught.exc, "winerror")?;
        if winerror.is_none_or(|value| value.is_none()) {
            let why = str_of(interp, &caught.exc)?;
            errors.push(Value::tuple(vec![src.clone(), dst.clone(), why]));
        }
    }
    if !errors.is_empty() {
        return Err(raise_global(interp, "Error", &[Value::list(errors)]));
    }
    Ok(dst.clone())
}

/// The body of `_copytree`'s `try` for one directory entry.
fn copy_entry_into(
    interp: &mut Interpreter,
    entry: &Value,
    srcname: &Value,
    dstname: &Value,
    srcobj: &Value,
    tree: &TreeArgs,
    nt: bool,
) -> Result<()> {
    let is_symlink_value = method(interp, entry, "is_symlink", &[])?;
    let mut is_symlink = truthy(interp, &is_symlink_value)?;
    if is_symlink && nt {
        // Special check for directory junctions, which appear as symlinks
        // but we want to recurse.
        let lstat = method_kw(
            interp,
            entry,
            "stat",
            &[],
            &[("follow_symlinks", Value::bool_(false))],
        )?;
        if let Some(tag) = getattr_opt(interp, &lstat, "st_reparse_tag")? {
            let stat = support::dep(interp, "stat")?;
            let mount_point = interp.get_attr(&stat, "IO_REPARSE_TAG_MOUNT_POINT")?;
            if equal(interp, &tag, &mount_point)? {
                is_symlink = false;
            }
        }
    }
    if is_symlink {
        let linkto = os_call(interp, "readlink", std::slice::from_ref(srcname))?;
        if truthy(interp, &tree.symlinks)? {
            // We can't just leave it to `copy_function` because legacy code
            // with a custom `copy_function` may rely on copytree doing the
            // right thing.
            os_call(interp, "symlink", &[linkto, dstname.clone()])?;
            let follow = !truthy(interp, &tree.symlinks)?;
            copystat(interp, srcobj, dstname, follow)?;
            return Ok(());
        }
        // ignore dangling symlink if the flag is on
        if !path_test(interp, "exists", &[linkto])?
            && truthy(interp, &tree.ignore_dangling_symlinks)?
        {
            return Ok(());
        }
        // otherwise let the copy occur. copy2 will raise an error
        let is_dir = method(interp, entry, "is_dir", &[])?;
        if truthy(interp, &is_dir)? {
            copytree(interp, srcobj, dstname, tree)?;
        } else {
            call(
                interp,
                tree.copy_function.clone(),
                &[srcobj.clone(), dstname.clone()],
            )?;
        }
        return Ok(());
    }
    let is_dir = method(interp, entry, "is_dir", &[])?;
    if truthy(interp, &is_dir)? {
        copytree(interp, srcobj, dstname, tree)?;
    } else {
        // Will raise a SpecialFileError for unsupported file types
        call(
            interp,
            tree.copy_function.clone(),
            &[srcobj.clone(), dstname.clone()],
        )?;
    }
    Ok(())
}
