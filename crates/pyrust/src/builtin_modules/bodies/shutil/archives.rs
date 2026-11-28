//! Archiving and unpacking of CPython 3.12.3 `Lib/shutil.py`: the format
//! registries (`_ARCHIVE_FORMATS`, `_UNPACK_FORMATS`) and their accessors,
//! `make_archive`, `unpack_archive`, and the built-in `_make_tarball`,
//! `_make_zipfile`, `_unpack_zipfile` and `_unpack_tarfile`, which drive the
//! runtime's `tarfile` and `zipfile` modules like the source does.
//!
//! The registries are ordinary dicts published as module attributes, so code
//! that edits `shutil._ARCHIVE_FORMATS` directly is seen too; the native
//! functions reach them through the per-interpreter state (`gil_local!`), so
//! one interpreter's registrations never leak into another.

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, Value};

use super::copy;
use super::support::{
    S_IFDIR, archive_formats, audit, binary, builtin, builtin_exc, call_dyn, call_kw, collect,
    contains, delitem, dep, equal, getattr_opt, getitem, method, method_kw, os_attr, os_call,
    path_call, path_test, percent, raise_builtin, raise_global, s_isfmt, setitem, sig, str_of,
    truthy, try_except, unpack_formats,
};
use super::system::{get_gid, get_uid};

const SIG_GET_ARCHIVE_FORMATS: Sig = sig("get_archive_formats", &[], 0, &[]);
const SIG_REGISTER_ARCHIVE: Sig = sig(
    "register_archive_format",
    &["name", "function", "extra_args", "description"],
    2,
    &[],
);
const SIG_UNREGISTER_ARCHIVE: Sig = sig("unregister_archive_format", &["name"], 0, &[]);
const SIG_MAKE_ARCHIVE: Sig = sig(
    "make_archive",
    &[
        "base_name",
        "format",
        "root_dir",
        "base_dir",
        "verbose",
        "dry_run",
        "owner",
        "group",
        "logger",
    ],
    7,
    &[],
);
const SIG_GET_UNPACK_FORMATS: Sig = sig("get_unpack_formats", &[], 0, &[]);
const SIG_CHECK_UNPACK: Sig = sig(
    "_check_unpack_options",
    &["extensions", "function", "extra_args"],
    0,
    &[],
);
const SIG_REGISTER_UNPACK: Sig = sig(
    "register_unpack_format",
    &[
        "name",
        "extensions",
        "function",
        "extra_args",
        "description",
    ],
    2,
    &[],
);
const SIG_UNREGISTER_UNPACK: Sig = sig("unregister_unpack_format", &["name"], 0, &[]);
const SIG_ENSURE_DIRECTORY: Sig = sig("_ensure_directory", &["path"], 0, &[]);
const SIG_UNPACK_ZIPFILE: Sig = sig("_unpack_zipfile", &["filename", "extract_dir"], 0, &[]);
const SIG_UNPACK_TARFILE: Sig = sig(
    "_unpack_tarfile",
    &["filename", "extract_dir"],
    0,
    &["filter"],
);
const SIG_FIND_UNPACK: Sig = sig("_find_unpack_format", &["filename"], 0, &[]);
const SIG_UNPACK_ARCHIVE: Sig = sig(
    "unpack_archive",
    &["filename", "extract_dir", "format"],
    2,
    &["filter"],
);
const SIG_MAKE_TARBALL: Sig = sig(
    "_make_tarball",
    &[
        "base_name",
        "base_dir",
        "compress",
        "verbose",
        "dry_run",
        "owner",
        "group",
        "logger",
        "root_dir",
    ],
    7,
    &[],
);
const SIG_MAKE_ZIPFILE: Sig = sig(
    "_make_zipfile",
    &[
        "base_name",
        "base_dir",
        "verbose",
        "dry_run",
        "logger",
        "owner",
        "group",
        "root_dir",
    ],
    6,
    &[],
);
const SIG_SET_UID_GID: Sig = sig("_make_tarball.<locals>._set_uid_gid", &["tarinfo"], 0, &[]);

// ---------------------------------------------------------------------------
// Registries
// ---------------------------------------------------------------------------

/// The initial `_ARCHIVE_FORMATS` and `_UNPACK_FORMATS` dicts (every
/// compression module is available, so all formats are registered).
pub(super) fn initial_registries(
    make_tarball: Value,
    make_zipfile: Value,
    unpack_tarfile: Value,
    unpack_zipfile: Value,
) -> (Value, Value) {
    let pair = |name: &str, value: Value| {
        Value::list(vec![Value::tuple(vec![Value::string(name), value])])
    };
    let mut archive = PyDict::default();
    for (name, function, extra, description) in [
        (
            "tar",
            make_tarball.clone(),
            pair("compress", Value::none()),
            "uncompressed tar file",
        ),
        (
            "gztar",
            make_tarball.clone(),
            pair("compress", Value::string("gzip")),
            "gzip'ed tar-file",
        ),
        ("zip", make_zipfile, Value::list(Vec::new()), "ZIP file"),
        (
            "bztar",
            make_tarball.clone(),
            pair("compress", Value::string("bzip2")),
            "bzip2'ed tar-file",
        ),
        (
            "xztar",
            make_tarball,
            pair("compress", Value::string("xz")),
            "xz'ed tar-file",
        ),
    ] {
        archive.insert(
            PyKey::str_from(name),
            Value::tuple(vec![function, extra, Value::string(description)]),
        );
    }

    let extensions = |exts: &[&str]| Value::list(exts.iter().copied().map(Value::string).collect());
    let mut unpack = PyDict::default();
    for (name, exts, function, description) in [
        (
            "tar",
            &[".tar"][..],
            unpack_tarfile.clone(),
            "uncompressed tar file",
        ),
        ("zip", &[".zip"][..], unpack_zipfile, "ZIP file"),
        (
            "gztar",
            &[".tar.gz", ".tgz"][..],
            unpack_tarfile.clone(),
            "gzip'ed tar-file",
        ),
        (
            "bztar",
            &[".tar.bz2", ".tbz2"][..],
            unpack_tarfile.clone(),
            "bzip2'ed tar-file",
        ),
        (
            "xztar",
            &[".tar.xz", ".txz"][..],
            unpack_tarfile,
            "xz'ed tar-file",
        ),
    ] {
        unpack.insert(
            PyKey::str_from(name),
            Value::tuple(vec![
                extensions(exts),
                function,
                Value::list(Vec::new()),
                Value::string(description),
            ]),
        );
    }
    (Value::dict(archive), Value::dict(unpack))
}

/// `registry.items()` as `(name, info)` pairs.
fn registry_items(interp: &mut Interpreter, registry: &Value) -> Result<Vec<(Value, Value)>> {
    let items = method(interp, registry, "items", &[])?;
    let mut pairs = Vec::new();
    for item in collect(interp, &items)? {
        let mut kv = collect(interp, &item)?.into_iter();
        if let (Some(key), Some(info)) = (kv.next(), kv.next()) {
            pairs.push((key, info));
        }
    }
    Ok(pairs)
}

/// `formats.sort(); return formats`.
fn sorted_list(interp: &mut Interpreter, formats: Vec<Value>) -> Result<Value> {
    let list = Value::list(formats);
    method(interp, &list, "sort", &[])?;
    Ok(list)
}

/// `get_archive_formats()`.
pub(super) fn get_archive_formats_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    args::bind(&SIG_GET_ARCHIVE_FORMATS, args)?;
    let registry = archive_formats()?;
    let mut formats = Vec::new();
    for (name, info) in registry_items(interp, &registry)? {
        let description = getitem(interp, &info, Value::int(2))?;
        formats.push(Value::tuple(vec![name, description]));
    }
    sorted_list(interp, formats)
}

/// `isinstance(value, (tuple, list))`.
fn is_tuple_or_list(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    Ok(super::support::isinstance_builtin(interp, value, "tuple")?
        || super::support::isinstance_builtin(interp, value, "list")?)
}

/// `register_archive_format(name, function, extra_args=None, description='')`.
pub(super) fn register_archive_format_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_REGISTER_ARCHIVE, args)?;
    let name = b.take_or_none(0);
    let function = b.take_or_none(1);
    let extra_args = b.take_or_none(2);
    let description = b.take_or(3, || Value::string(""));
    let extra_args = if extra_args.is_none() {
        Value::list(Vec::new())
    } else {
        extra_args
    };
    let callable = builtin(interp, "callable", std::slice::from_ref(&function))?;
    if !truthy(interp, &callable)? {
        let msg = percent(interp, "The %s object is not callable", vec![function])?;
        return Err(raise_builtin(interp, "TypeError", &[msg]));
    }
    if !is_tuple_or_list(interp, &extra_args)? {
        return Err(raise_builtin(
            interp,
            "TypeError",
            &[Value::string("extra_args needs to be a sequence")],
        ));
    }
    for element in collect(interp, &extra_args)? {
        let pair = is_tuple_or_list(interp, &element)?;
        let len_ok = pair && {
            let len = builtin(interp, "len", std::slice::from_ref(&element))?;
            len.as_int() == Some(2)
        };
        if !len_ok {
            return Err(raise_builtin(
                interp,
                "TypeError",
                &[Value::string("extra_args elements are : (arg_name, value)")],
            ));
        }
    }
    let registry = archive_formats()?;
    setitem(
        interp,
        &registry,
        name,
        Value::tuple(vec![function, extra_args, description]),
    )?;
    Ok(Value::none())
}

/// `unregister_archive_format(name)`.
pub(super) fn unregister_archive_format_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_UNREGISTER_ARCHIVE, args)?;
    let name = b.take_or_none(0);
    let registry = archive_formats()?;
    delitem(interp, &registry, name)?;
    Ok(Value::none())
}

/// `kwargs[name] = value`, keeping the first insertion position.
fn set_kwarg(kwargs: &mut Vec<(String, Value)>, name: &str, value: Value) {
    match kwargs.iter_mut().find(|(key, _)| key == name) {
        Some(slot) => slot.1 = value,
        None => kwargs.push((name.to_string(), value)),
    }
}

/// The keyword name of an `extra_args` pair (`func(**kwargs)` needs `str`).
fn keyword_name(value: &Value) -> Result<String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| PyError::named("TypeError", "keywords must be strings".to_string()))
}

/// `make_archive(base_name, format, root_dir=None, base_dir=None, verbose=0,
/// dry_run=0, owner=None, group=None, logger=None)`.
pub(super) fn make_archive_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_MAKE_ARCHIVE, args)?;
    let mut base_name = b.take_or_none(0);
    let format = b.take_or_none(1);
    let root_dir = b.take_or_none(2);
    let base_dir = b.take_or_none(3);
    let _verbose = b.take_or(4, || Value::int(0));
    let dry_run = b.take_or(5, || Value::int(0));
    let owner = b.take_or_none(6);
    let group = b.take_or_none(7);
    let logger = b.take_or_none(8);
    audit(
        interp,
        "shutil.make_archive",
        &[
            base_name.clone(),
            format.clone(),
            root_dir.clone(),
            base_dir.clone(),
        ],
    )?;

    let registry = archive_formats()?;
    let key_error = builtin_exc("KeyError")?;
    let format_info = match try_except(interp, &[key_error], |interp| {
        getitem(interp, &registry, format.clone())
    })? {
        Ok(info) => info,
        Err(_) => {
            let msg = percent(interp, "unknown archive format '%s'", vec![format])?;
            return Err(raise_builtin(interp, "ValueError", &[msg]));
        }
    };

    let mut kwargs = vec![
        ("dry_run".to_string(), dry_run.clone()),
        ("logger".to_string(), logger.clone()),
        ("owner".to_string(), owner),
        ("group".to_string(), group),
    ];
    let func = getitem(interp, &format_info, Value::int(0))?;
    let extra = getitem(interp, &format_info, Value::int(1))?;
    for pair in collect(interp, &extra)? {
        let [arg, val]: [Value; 2] = collect(interp, &pair)?.try_into().map_err(|_| {
            PyError::named(
                "ValueError",
                "too many values to unpack (expected 2)".to_string(),
            )
        })?;
        set_kwarg(&mut kwargs, &keyword_name(&arg)?, val);
    }

    let base_dir = if base_dir.is_none() {
        os_attr(interp, "curdir")?
    } else {
        base_dir
    };

    let supports_root_dir = match getattr_opt(interp, &func, "supports_root_dir")? {
        Some(value) => truthy(interp, &value)?,
        None => false,
    };
    let mut save_cwd = None;
    if !root_dir.is_none() {
        let st = os_call(interp, "stat", std::slice::from_ref(&root_dir))?;
        let mode = interp.get_attr(&st, "st_mode")?;
        if !s_isfmt(interp, &mode, S_IFDIR)? {
            let errno = dep(interp, "errno")?;
            let enotdir = interp.get_attr(&errno, "ENOTDIR")?;
            return Err(raise_builtin(
                interp,
                "NotADirectoryError",
                &[enotdir, Value::string("Not a directory"), root_dir],
            ));
        }
        if supports_root_dir {
            // Support path-like base_name here for backwards-compatibility.
            base_name = os_call(interp, "fspath", &[base_name])?;
            set_kwarg(&mut kwargs, "root_dir", root_dir);
        } else {
            save_cwd = Some(os_call(interp, "getcwd", &[])?);
            if !logger.is_none() {
                let msg = Value::string("changing into '%s'");
                method(interp, &logger, "debug", &[msg, root_dir.clone()])?;
            }
            base_name = path_call(interp, "abspath", &[base_name])?;
            if !truthy(interp, &dry_run)? {
                os_call(interp, "chdir", &[root_dir])?;
            }
        }
    }

    let filename = call_dyn(interp, func, &[base_name, base_dir], kwargs);
    if let Some(save_cwd) = save_cwd {
        if !logger.is_none() {
            let msg = Value::string("changing back to '%s'");
            method(interp, &logger, "debug", &[msg, save_cwd.clone()])?;
        }
        os_call(interp, "chdir", &[save_cwd])?;
    }
    filename
}

/// `get_unpack_formats()`.
pub(super) fn get_unpack_formats_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    args::bind(&SIG_GET_UNPACK_FORMATS, args)?;
    let registry = unpack_formats()?;
    let mut formats = Vec::new();
    for (name, info) in registry_items(interp, &registry)? {
        let extensions = getitem(interp, &info, Value::int(0))?;
        let description = getitem(interp, &info, Value::int(3))?;
        formats.push(Value::tuple(vec![name, extensions, description]));
    }
    sorted_list(interp, formats)
}

/// `_check_unpack_options(extensions, function, extra_args)`.
fn check_unpack_options(
    interp: &mut Interpreter,
    extensions: &Value,
    function: &Value,
) -> Result<()> {
    // first make sure no other unpacker is registered for this extension
    let existing = Value::dict(PyDict::default());
    let registry = unpack_formats()?;
    for (name, info) in registry_items(interp, &registry)? {
        let exts = getitem(interp, &info, Value::int(0))?;
        for ext in collect(interp, &exts)? {
            setitem(interp, &existing, ext, name.clone())?;
        }
    }
    for extension in collect(interp, extensions)? {
        if contains(interp, &existing, &extension)? {
            let owner = getitem(interp, &existing, extension.clone())?;
            let msg = percent(
                interp,
                "%s is already registered for \"%s\"",
                vec![extension, owner],
            )?;
            return Err(raise_global(interp, "RegistryError", &[msg]));
        }
    }
    let callable = builtin(interp, "callable", std::slice::from_ref(function))?;
    if !truthy(interp, &callable)? {
        return Err(raise_builtin(
            interp,
            "TypeError",
            &[Value::string("The registered function must be a callable")],
        ));
    }
    Ok(())
}

pub(super) fn check_unpack_options_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_CHECK_UNPACK, args)?;
    let extensions = b.take_or_none(0);
    let function = b.take_or_none(1);
    check_unpack_options(interp, &extensions, &function)?;
    Ok(Value::none())
}

/// `register_unpack_format(name, extensions, function, extra_args=None,
/// description='')`.
pub(super) fn register_unpack_format_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_REGISTER_UNPACK, args)?;
    let name = b.take_or_none(0);
    let extensions = b.take_or_none(1);
    let function = b.take_or_none(2);
    let extra_args = b.take_or_none(3);
    let description = b.take_or(4, || Value::string(""));
    let extra_args = if extra_args.is_none() {
        Value::list(Vec::new())
    } else {
        extra_args
    };
    check_unpack_options(interp, &extensions, &function)?;
    let registry = unpack_formats()?;
    setitem(
        interp,
        &registry,
        name,
        Value::tuple(vec![extensions, function, extra_args, description]),
    )?;
    Ok(Value::none())
}

/// `unregister_unpack_format(name)`.
pub(super) fn unregister_unpack_format_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_UNREGISTER_UNPACK, args)?;
    let name = b.take_or_none(0);
    let registry = unpack_formats()?;
    delitem(interp, &registry, name)?;
    Ok(Value::none())
}

/// `_ensure_directory(path)`: make sure the parent directory exists.
fn ensure_directory(interp: &mut Interpreter, path: &Value) -> Result<()> {
    let dirname = path_call(interp, "dirname", std::slice::from_ref(path))?;
    if !path_test(interp, "isdir", std::slice::from_ref(&dirname))? {
        os_call(interp, "makedirs", &[dirname])?;
    }
    Ok(())
}

pub(super) fn ensure_directory_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_ENSURE_DIRECTORY, args)?;
    let path = b.take_or_none(0);
    ensure_directory(interp, &path)?;
    Ok(Value::none())
}

/// `_unpack_zipfile(filename, extract_dir)`.
pub(super) fn unpack_zipfile_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_UNPACK_ZIPFILE, args)?;
    let filename = b.take_or_none(0);
    let extract_dir = b.take_or_none(1);
    // late import for breaking circular dependency
    let zipfile = interp.load_module("zipfile")?;
    let is_zip = method(
        interp,
        &zipfile,
        "is_zipfile",
        std::slice::from_ref(&filename),
    )?;
    if !truthy(interp, &is_zip)? {
        let msg = percent(interp, "%s is not a zip file", vec![filename])?;
        return Err(raise_global(interp, "ReadError", &[msg]));
    }
    let zip = method(interp, &zipfile, "ZipFile", &[filename])?;
    let extracted = extract_zip_members(interp, &zip, &extract_dir);
    let closed = method(interp, &zip, "close", &[]);
    extracted?;
    closed?;
    Ok(Value::none())
}

fn extract_zip_members(interp: &mut Interpreter, zip: &Value, extract_dir: &Value) -> Result<()> {
    let infos = method(interp, zip, "infolist", &[])?;
    for info in collect(interp, &infos)? {
        let name = interp.get_attr(&info, "filename")?;
        // don't extract absolute paths or ones with .. in them
        let absolute = method(interp, &name, "startswith", &[Value::string("/")])?;
        if truthy(interp, &absolute)? || contains(interp, &name, &Value::string(".."))? {
            continue;
        }
        let parts = method(interp, &name, "split", &[Value::string("/")])?;
        let mut join_args = vec![extract_dir.clone()];
        join_args.extend(collect(interp, &parts)?);
        let targetpath = path_call(interp, "join", &join_args)?;
        if !truthy(interp, &targetpath)? {
            continue;
        }
        ensure_directory(interp, &targetpath)?;
        let is_dir = method(interp, &name, "endswith", &[Value::string("/")])?;
        if truthy(interp, &is_dir)? {
            continue;
        }
        // file
        let source = method(interp, zip, "open", &[name, Value::string("r")])?;
        let written = (|| -> Result<()> {
            let target = builtin(interp, "open", &[targetpath, Value::string("wb")])?;
            let copied = copy::copyfileobj(interp, &source, &target, Value::int(0));
            let closed = method(interp, &target, "close", &[]);
            copied?;
            closed?;
            Ok(())
        })();
        let closed = method(interp, &source, "close", &[]);
        written?;
        closed?;
    }
    Ok(())
}

/// `_unpack_tarfile(filename, extract_dir, *, filter=None)`.
pub(super) fn unpack_tarfile_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_UNPACK_TARFILE, args)?;
    let filename = b.take_or_none(0);
    let extract_dir = b.take_or_none(1);
    let filter = b.take_or_none(2);
    // late import for breaking circular dependency
    let tarfile = interp.load_module("tarfile")?;
    let tar_error = match interp.get_attr(&tarfile, "TarError")?.kind() {
        crate::value::ValueKind::PyClass(class) => Rc::clone(class),
        _ => builtin_exc("Exception")?,
    };
    let opened = try_except(interp, &[tar_error], |interp| {
        method(interp, &tarfile, "open", std::slice::from_ref(&filename))
    })?;
    let tarobj = match opened {
        Ok(tarobj) => tarobj,
        Err(_) => {
            let msg = percent(
                interp,
                "%s is not a compressed or uncompressed tar file",
                vec![filename],
            )?;
            return Err(raise_global(interp, "ReadError", &[msg]));
        }
    };
    let extracted = method_kw(
        interp,
        &tarobj,
        "extractall",
        &[extract_dir],
        &[("filter", filter)],
    );
    let closed = method(interp, &tarobj, "close", &[]);
    extracted?;
    closed?;
    Ok(Value::none())
}

/// `_find_unpack_format(filename)`.
fn find_unpack_format(interp: &mut Interpreter, filename: &Value) -> Result<Value> {
    let registry = unpack_formats()?;
    for (name, info) in registry_items(interp, &registry)? {
        let exts = getitem(interp, &info, Value::int(0))?;
        for extension in collect(interp, &exts)? {
            let matched = method(interp, filename, "endswith", &[extension])?;
            if truthy(interp, &matched)? {
                return Ok(name);
            }
        }
    }
    Ok(Value::none())
}

pub(super) fn find_unpack_format_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_FIND_UNPACK, args)?;
    let filename = b.take_or_none(0);
    find_unpack_format(interp, &filename)
}

/// `dict(pairs)` as keyword arguments.
fn kwargs_of(interp: &mut Interpreter, pairs: &Value) -> Result<Vec<(String, Value)>> {
    let mut kwargs = Vec::new();
    let as_dict = builtin(interp, "dict", std::slice::from_ref(pairs))?;
    for (key, value) in registry_items(interp, &as_dict)? {
        kwargs.push((keyword_name(&key)?, value));
    }
    Ok(kwargs)
}

/// `unpack_archive(filename, extract_dir=None, format=None, *, filter=None)`.
pub(super) fn unpack_archive_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_UNPACK_ARCHIVE, args)?;
    let filename = b.take_or_none(0);
    let extract_dir = b.take_or_none(1);
    let format = b.take_or_none(2);
    let filter = b.take_or_none(3);
    audit(
        interp,
        "shutil.unpack_archive",
        &[filename.clone(), extract_dir.clone(), format.clone()],
    )?;

    let extract_dir = if extract_dir.is_none() {
        os_call(interp, "getcwd", &[])?
    } else {
        extract_dir
    };
    let extract_dir = os_call(interp, "fspath", &[extract_dir])?;
    let filename = os_call(interp, "fspath", &[filename])?;

    let registry = unpack_formats()?;
    let (func, mut kwargs) = if format.is_none() {
        // we need to look at the registered unpackers supported extensions
        let format = find_unpack_format(interp, &filename)?;
        if format.is_none() {
            let shown = str_of(interp, &filename)?;
            let msg = format!(
                "Unknown archive format '{}'",
                shown.as_str().unwrap_or_default()
            );
            return Err(raise_global(interp, "ReadError", &[Value::string(msg)]));
        }
        let info = getitem(interp, &registry, format)?;
        let func = getitem(interp, &info, Value::int(1))?;
        let extra = getitem(interp, &info, Value::int(2))?;
        (func, kwargs_of(interp, &extra)?)
    } else {
        let key_error = builtin_exc("KeyError")?;
        let info = match try_except(interp, &[key_error], |interp| {
            getitem(interp, &registry, format.clone())
        })? {
            Ok(info) => info,
            Err(_) => {
                let shown = str_of(interp, &format)?;
                let msg = format!(
                    "Unknown unpack format '{}'",
                    shown.as_str().unwrap_or_default()
                );
                return Err(raise_builtin(interp, "ValueError", &[Value::string(msg)]));
            }
        };
        let func = getitem(interp, &info, Value::int(1))?;
        let extra = getitem(interp, &info, Value::int(2))?;
        (func, kwargs_of(interp, &extra)?)
    };
    if !filter.is_none() {
        if format.is_none() {
            // dict(...) | filter_kwargs
            set_kwarg(&mut kwargs, "filter", filter);
        } else if kwargs.iter().any(|(key, _)| key == "filter") {
            let qualname = getattr_opt(interp, &func, "__qualname__")?
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "function".to_string());
            return Err(PyError::named(
                "TypeError",
                format!("{qualname}() got multiple values for keyword argument 'filter'"),
            ));
        } else {
            kwargs.push(("filter".to_string(), filter));
        }
    }
    call_dyn(interp, func, &[filename, extract_dir], kwargs)?;
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// _make_tarball / _make_zipfile
// ---------------------------------------------------------------------------

/// `if archive_dir and not os.path.exists(archive_dir): ... os.makedirs`.
fn create_archive_dir(
    interp: &mut Interpreter,
    archive_dir: &Value,
    dry_run: &Value,
    logger: &Value,
) -> Result<()> {
    if !truthy(interp, archive_dir)?
        || path_test(interp, "exists", std::slice::from_ref(archive_dir))?
    {
        return Ok(());
    }
    if !logger.is_none() {
        method(
            interp,
            logger,
            "info",
            &[Value::string("creating %s"), archive_dir.clone()],
        )?;
    }
    if !truthy(interp, dry_run)? {
        os_call(interp, "makedirs", std::slice::from_ref(archive_dir))?;
    }
    Ok(())
}

/// The `uid` / `gid` the `_set_uid_gid` filter applies.
pub(super) struct OwnerFilterState {
    uid: Value,
    gid: Value,
    owner: Value,
    group: Value,
}

pub(super) const OWNER_FILTER_TYPE: &str = "_set_uid_gid";

/// `_make_tarball.<locals>._set_uid_gid(tarinfo)`.
pub(super) fn owner_filter_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, rest) = args.split_first().ok_or_else(|| {
        args::type_error("_set_uid_gid() missing 1 required positional argument: 'tarinfo'")
    })?;
    let (uid, gid, owner, group) = crate::builtin_modules::macros::native_state::with_native_state(
        &this.value,
        OWNER_FILTER_TYPE,
        |state: &OwnerFilterState| {
            Ok((
                state.uid.clone(),
                state.gid.clone(),
                state.owner.clone(),
                state.group.clone(),
            ))
        },
    )?;
    let mut b = args::bind(&SIG_SET_UID_GID, rest)?;
    let tarinfo = b.take_or_none(0);
    if !gid.is_none() {
        interp.assign_attr(tarinfo.clone(), "gid", gid)?;
        interp.assign_attr(tarinfo.clone(), "gname", group)?;
    }
    if !uid.is_none() {
        interp.assign_attr(tarinfo.clone(), "uid", uid)?;
        interp.assign_attr(tarinfo.clone(), "uname", owner)?;
    }
    Ok(tarinfo)
}

fn owner_filter(uid: Value, gid: Value, owner: Value, group: Value) -> Result<Value> {
    let class = super::support::owner_filter_class()?;
    let filter = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    crate::builtin_modules::macros::native_state::set_native_state(
        &filter,
        OwnerFilterState {
            uid,
            gid,
            owner,
            group,
        },
    )?;
    Ok(filter)
}

/// `_make_tarball(base_name, base_dir, compress="gzip", verbose=0,
/// dry_run=0, owner=None, group=None, logger=None, root_dir=None)`.
pub(super) fn make_tarball_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_MAKE_TARBALL, args)?;
    let base_name = b.take_or_none(0);
    let base_dir = b.take_or_none(1);
    let compress = b.take_or(2, || Value::string("gzip"));
    let _verbose = b.take_or(3, || Value::int(0));
    let dry_run = b.take_or(4, || Value::int(0));
    let owner = b.take_or_none(5);
    let group = b.take_or_none(6);
    let logger = b.take_or_none(7);
    let root_dir = b.take_or_none(8);

    let tar_compression = if compress.is_none() {
        ""
    } else if equal(interp, &compress, &Value::string("gzip"))? {
        "gz"
    } else if equal(interp, &compress, &Value::string("bzip2"))? {
        "bz2"
    } else if equal(interp, &compress, &Value::string("xz"))? {
        "xz"
    } else {
        let shown = str_of(interp, &compress)?;
        let msg = format!(
            "bad value for 'compress', or compression format not supported : {}",
            shown.as_str().unwrap_or_default()
        );
        return Err(raise_builtin(interp, "ValueError", &[Value::string(msg)]));
    };

    // late import for breaking circular dependency
    let tarfile = interp.load_module("tarfile")?;

    let compress_ext = if truthy(interp, &compress)? {
        format!(".{tar_compression}")
    } else {
        String::new()
    };
    let suffix = Value::string(format!(".tar{compress_ext}"));
    let mut archive_name = binary(interp, &base_name, BinaryOp::Add, &suffix)?;
    let archive_dir = path_call(interp, "dirname", std::slice::from_ref(&archive_name))?;
    create_archive_dir(interp, &archive_dir, &dry_run, &logger)?;

    // creating the tarball
    if !logger.is_none() {
        method(
            interp,
            &logger,
            "info",
            &[Value::string("Creating tar archive")],
        )?;
    }

    let uid = get_uid(interp, &owner)?;
    let gid = get_gid(interp, &group)?;
    let filter = if uid.is_none() && gid.is_none() {
        // `_set_uid_gid` would hand every member back unchanged.
        Value::none()
    } else {
        owner_filter(uid, gid, owner, group)?
    };

    if !truthy(interp, &dry_run)? {
        let mode = Value::string(format!("w|{tar_compression}"));
        let tar = method(interp, &tarfile, "open", &[archive_name.clone(), mode])?;
        let arcname = base_dir.clone();
        let added = (|| -> Result<()> {
            let base_dir = if root_dir.is_none() {
                base_dir
            } else {
                path_call(interp, "join", &[root_dir.clone(), base_dir])?
            };
            method_kw(
                interp,
                &tar,
                "add",
                &[base_dir, arcname],
                &[("filter", filter)],
            )?;
            Ok(())
        })();
        let closed = method(interp, &tar, "close", &[]);
        added?;
        closed?;
    }

    if !root_dir.is_none() {
        archive_name = path_call(interp, "abspath", &[archive_name])?;
    }
    Ok(archive_name)
}

/// `_make_zipfile(base_name, base_dir, verbose=0, dry_run=0, logger=None,
/// owner=None, group=None, root_dir=None)`.
pub(super) fn make_zipfile_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_MAKE_ZIPFILE, args)?;
    let base_name = b.take_or_none(0);
    let base_dir = b.take_or_none(1);
    let _verbose = b.take_or(2, || Value::int(0));
    let dry_run = b.take_or(3, || Value::int(0));
    let logger = b.take_or_none(4);
    let _owner = b.take_or_none(5);
    let _group = b.take_or_none(6);
    let root_dir = b.take_or_none(7);

    // late import for breaking circular dependency
    let zipfile = interp.load_module("zipfile")?;

    let mut zip_filename = binary(interp, &base_name, BinaryOp::Add, &Value::string(".zip"))?;
    let archive_dir = path_call(interp, "dirname", std::slice::from_ref(&base_name))?;
    create_archive_dir(interp, &archive_dir, &dry_run, &logger)?;

    if !logger.is_none() {
        method(
            interp,
            &logger,
            "info",
            &[
                Value::string("creating '%s' and adding '%s' to it"),
                zip_filename.clone(),
                base_dir.clone(),
            ],
        )?;
    }

    if !truthy(interp, &dry_run)? {
        let compression = interp.get_attr(&zipfile, "ZIP_DEFLATED")?;
        let zip_class = interp.get_attr(&zipfile, "ZipFile")?;
        let zf = call_kw(
            interp,
            zip_class,
            &[zip_filename.clone(), Value::string("w")],
            &[("compression", compression)],
        )?;
        let written = write_zip_tree(interp, &zf, &base_dir, &root_dir, &logger);
        let closed = method(interp, &zf, "close", &[]);
        written?;
        closed?;
    }

    if !root_dir.is_none() {
        zip_filename = path_call(interp, "abspath", &[zip_filename])?;
    }
    Ok(zip_filename)
}

/// `logger.info("adding '%s'", path)` when a logger is given.
fn log_adding(interp: &mut Interpreter, logger: &Value, path: &Value) -> Result<()> {
    if !logger.is_none() {
        method(
            interp,
            logger,
            "info",
            &[Value::string("adding '%s'"), path.clone()],
        )?;
    }
    Ok(())
}

/// The body of `_make_zipfile`'s `with zipfile.ZipFile(...) as zf:`.
fn write_zip_tree(
    interp: &mut Interpreter,
    zf: &Value,
    base_dir: &Value,
    root_dir: &Value,
    logger: &Value,
) -> Result<()> {
    let arcname = path_call(interp, "normpath", std::slice::from_ref(base_dir))?;
    let base_dir = if root_dir.is_none() {
        base_dir.clone()
    } else {
        path_call(interp, "join", &[root_dir.clone(), base_dir.clone()])?
    };
    let base_dir = path_call(interp, "normpath", &[base_dir])?;
    let curdir = os_attr(interp, "curdir")?;
    if !equal(interp, &arcname, &curdir)? {
        method(interp, zf, "write", &[base_dir.clone(), arcname])?;
        log_adding(interp, logger, &base_dir)?;
    }
    let walk = os_call(interp, "walk", std::slice::from_ref(&base_dir))?;
    for step in collect(interp, &walk)? {
        let [dirpath, dirnames, filenames]: [Value; 3] =
            collect(interp, &step)?.try_into().map_err(|_| {
                PyError::named("ValueError", "os.walk() yielded a bad triple".to_string())
            })?;
        let mut arcdirpath = dirpath.clone();
        if !root_dir.is_none() {
            arcdirpath = path_call(interp, "relpath", &[arcdirpath, root_dir.clone()])?;
        }
        let arcdirpath = path_call(interp, "normpath", &[arcdirpath])?;
        let dirnames = builtin(interp, "sorted", &[dirnames])?;
        for name in collect(interp, &dirnames)? {
            let path = path_call(interp, "join", &[dirpath.clone(), name.clone()])?;
            let arcname = path_call(interp, "join", &[arcdirpath.clone(), name])?;
            method(interp, zf, "write", &[path.clone(), arcname])?;
            log_adding(interp, logger, &path)?;
        }
        for name in collect(interp, &filenames)? {
            let path = path_call(interp, "join", &[dirpath.clone(), name.clone()])?;
            let path = path_call(interp, "normpath", &[path])?;
            if path_test(interp, "isfile", std::slice::from_ref(&path))? {
                let arcname = path_call(interp, "join", &[arcdirpath.clone(), name])?;
                method(interp, zf, "write", &[path.clone(), arcname])?;
                log_adding(interp, logger, &path)?;
            }
        }
    }
    Ok(())
}
