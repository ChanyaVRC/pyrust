// `glob` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/glob.py` (issues #3148, #3463).
//
// The pure-Python module is a stack of nested generators (`_iglob` over
// `_glob0` / `_glob1` / `_glob2` / `_rlistdir`).  Here the same call tree is an
// explicit state machine (`IglobState` / `Glob2` / `Rlist`), so `iglob()`
// stays a lazy iterator: a directory is only listed once the consumer asks for
// the first name that needs it, a recursive `**` walk descends one directory
// at a time, and nothing below a yielded path is scanned before it is
// resumed.  File names are matched with the native `fnmatch` matcher
// (`fnmatch::filter_path_names`) and the path arithmetic uses the same
// `split` / `join` primitives as the native `os.path` (`posixpath` on POSIX,
// `ntpath` on Windows), so drive letters, UNC paths and `altsep` behave as in
// CPython on both platforms.
//
// What this port deliberately does not do:
// - `dir_fd` needs `os.open(..., dir_fd=)` / `os.scandir(fd)`, which this
//   runtime does not provide.  Like CPython on Windows, passing a `dir_fd`
//   raises `NotImplementedError` as soon as the filesystem is touched.
// - `glob.magic_check` / `magic_check_bytes` (compiled `re` patterns) are not
//   exported: the module never imports `re`.
//
// Reference: <https://docs.python.org/3.12/library/glob.html>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::genericpath::ops::{
    PathValue, extract_str_or_bytes, fspath_resolve, path_buf_to_bytes,
};
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};
use crate::interpreter::{ExpandedCallArg, Interpreter, ProviderIterator, value_type_name_str};
use crate::value::Value;

#[cfg(windows)]
use crate::builtin_modules::ntpath::ops::{
    nt_join_bytes as host_join_bytes, nt_join_str as host_join_str,
    nt_split_bytes as host_split_bytes, nt_split_str as host_split_str,
    nt_splitdrive_bytes as host_splitdrive_bytes, nt_splitdrive_str as host_splitdrive_str,
};
#[cfg(not(windows))]
use crate::builtin_modules::posixpath::ops::{
    posix_join_bytes as host_join_bytes, posix_join_str as host_join_str,
    posix_split_bytes as host_split_bytes, posix_split_str as host_split_str,
};

const MODULE_DOC: &str = "Filename globbing utility.";

// The `def`s of `Lib/glob.py`: CPython reports arity errors for these in the
// interpreter's `initialize_locals` wording, not the macro's.
const SIG_GLOB: Sig = Sig {
    qualname: "glob",
    params: &["pathname"],
    defaults: 0,
    kwonly: &["root_dir", "dir_fd", "recursive", "include_hidden"],
    var_kw: false,
};
const SIG_IGLOB: Sig = Sig {
    qualname: "iglob",
    params: &["pathname"],
    defaults: 0,
    kwonly: &["root_dir", "dir_fd", "recursive", "include_hidden"],
    var_kw: false,
};
const SIG_GLOB0: Sig = Sig {
    qualname: "glob0",
    params: &["dirname", "pattern"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_GLOB1: Sig = Sig {
    qualname: "glob1",
    params: &["dirname", "pattern"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_HAS_MAGIC: Sig = Sig {
    qualname: "has_magic",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ESCAPE: Sig = Sig {
    qualname: "escape",
    params: &["pathname"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn all_list() -> Value {
    Value::list(
        ["glob", "iglob", "escape"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// `str` / `bytes` path operands
// ---------------------------------------------------------------------------

fn is_empty(p: &PathValue) -> bool {
    match p {
        PathValue::Str(s) => s.is_empty(),
        PathValue::Bytes(b) => b.is_empty(),
    }
}

/// `p[:0]`.
fn empty_like(p: &PathValue) -> PathValue {
    match p {
        PathValue::Str(_) => PathValue::Str(String::new()),
        PathValue::Bytes(_) => PathValue::Bytes(Vec::new()),
    }
}

fn same_path(a: &PathValue, b: &PathValue) -> bool {
    match (a, b) {
        (PathValue::Str(a), PathValue::Str(b)) => a == b,
        (PathValue::Bytes(a), PathValue::Bytes(b)) => a == b,
        _ => false,
    }
}

/// `magic_check.search(s) is not None` (`([*?[])`).
fn has_magic_pv(p: &PathValue) -> bool {
    match p {
        PathValue::Str(s) => s.contains(['*', '?', '[']),
        PathValue::Bytes(b) => b.iter().any(|c| matches!(c, b'*' | b'?' | b'[')),
    }
}

/// `_isrecursive(pattern)`.
fn is_recursive(p: &PathValue) -> bool {
    match p {
        PathValue::Str(s) => s == "**",
        PathValue::Bytes(b) => b == b"**",
    }
}

/// `_ishidden(path)`: `path[0] in ('.', b'.'[0])`, an `IndexError` for `''`.
fn ishidden(p: &PathValue) -> Result<bool> {
    match p {
        PathValue::Str(s) => {
            s.chars().next().map(|c| c == '.').ok_or_else(|| {
                PyError::named("IndexError", "string index out of range".to_string())
            })
        }
        PathValue::Bytes(b) => b
            .first()
            .map(|&c| c == b'.')
            .ok_or_else(|| PyError::named("IndexError", "index out of range".to_string())),
    }
}

/// `os.path.split(p)`.
fn split_pv(p: &PathValue) -> (PathValue, PathValue) {
    match p {
        PathValue::Str(s) => {
            let (head, tail) = host_split_str(s);
            (PathValue::Str(head), PathValue::Str(tail))
        }
        PathValue::Bytes(b) => {
            let (head, tail) = host_split_bytes(b);
            (PathValue::Bytes(head), PathValue::Bytes(tail))
        }
    }
}

/// `os.path.join(a, b)`.
fn join_strict(a: &PathValue, b: &PathValue) -> Result<PathValue> {
    match (a, b) {
        (PathValue::Str(a), PathValue::Str(b)) => {
            Ok(PathValue::Str(host_join_str(&[a.clone(), b.clone()])))
        }
        (PathValue::Bytes(a), PathValue::Bytes(b)) => {
            Ok(PathValue::Bytes(host_join_bytes(&[a.clone(), b.clone()])))
        }
        _ => Err(type_error("Can't mix strings and bytes in path components")),
    }
}

/// `_join(dirname, basename)`: an empty side is dropped without type checks.
fn join_(dirname: &PathValue, basename: &PathValue) -> Result<PathValue> {
    if is_empty(dirname) || is_empty(basename) {
        return Ok(if is_empty(dirname) {
            basename.clone()
        } else {
            dirname.clone()
        });
    }
    join_strict(dirname, basename)
}

// ---------------------------------------------------------------------------
// Filesystem queries
// ---------------------------------------------------------------------------

/// One glob call's options.
struct Ctx {
    root_dir: PathValue,
    recursive: bool,
    include_hidden: bool,
    /// `dir_fd is not None`.
    dir_fd: bool,
}

impl Ctx {
    /// The context `glob0()` / `glob1()` run in: `(…, None, False)`.
    fn bare(root_dir: PathValue) -> Self {
        Self {
            root_dir,
            recursive: false,
            include_hidden: false,
            dir_fd: false,
        }
    }

    fn check_dir_fd(&self) -> Result<()> {
        if self.dir_fd {
            // What `os.lstat(path, dir_fd=fd)` raises on Windows.
            return Err(PyError::named(
                "NotImplementedError",
                "dir_fd unavailable on this platform".to_string(),
            ));
        }
        Ok(())
    }
}

/// `os.path.lexists()` (an unusable path is simply absent).
fn lexists(ctx: &Ctx, path: &PathValue) -> Result<bool> {
    ctx.check_dir_fd()?;
    Ok(path
        .to_path_buf()
        .is_ok_and(|p| std::fs::symlink_metadata(p).is_ok()))
}

/// `os.path.isdir()`.
fn isdir(ctx: &Ctx, path: &PathValue) -> Result<bool> {
    ctx.check_dir_fd()?;
    Ok(path
        .to_path_buf()
        .is_ok_and(|p| std::fs::metadata(p).is_ok_and(|m| m.is_dir())))
}

/// `entry.is_dir()` (follows symlinks); an unreadable entry is not a directory.
fn entry_is_dir(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_dir() => true,
        Ok(kind) if kind.is_symlink() => std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()),
        _ => false,
    }
}

/// `_listdir(dirname, dir_fd, dironly)`: the entry names of a directory in
/// `scandir` order, only the directories when `dironly`.  An `OSError` (a
/// missing or unreadable directory, a failure while reading) ends the listing
/// quietly.
fn listdir(ctx: &Ctx, dirname: &PathValue, dironly: bool) -> Result<Vec<PathValue>> {
    ctx.check_dir_fd()?;
    let path = if is_empty(dirname) {
        std::path::PathBuf::from(".")
    } else {
        dirname.to_path_buf()?
    };
    let Ok(entries) = std::fs::read_dir(&path) else {
        return Ok(Vec::new());
    };
    let is_bytes = matches!(dirname, PathValue::Bytes(_));
    let mut names = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { break };
        if dironly && !entry_is_dir(&entry) {
            continue;
        }
        let name = entry.file_name();
        names.push(if is_bytes {
            PathValue::Bytes(path_buf_to_bytes(std::path::Path::new(&name)))
        } else {
            PathValue::Str(name.to_string_lossy().into_owned())
        });
    }
    Ok(names)
}

// ---------------------------------------------------------------------------
// `_glob0` / `_glob1`: one literal directory, non-recursive
// ---------------------------------------------------------------------------

/// `_glob1(dirname, pattern, dir_fd, dironly, include_hidden)`.
fn glob1_in(
    interp: &mut Interpreter,
    ctx: &Ctx,
    dirname: &PathValue,
    pattern: &PathValue,
    dironly: bool,
) -> Result<Vec<PathValue>> {
    let mut names = listdir(ctx, dirname, dironly)?;
    if !ctx.include_hidden && !ishidden(pattern)? {
        let mut visible = Vec::with_capacity(names.len());
        for name in names {
            if !ishidden(&name)? {
                visible.push(name);
            }
        }
        names = visible;
    }
    crate::builtin_modules::fnmatch::filter_path_names(interp, pattern, names)
}

/// `_glob0(dirname, basename, dir_fd, dironly)`.
fn glob0_in(ctx: &Ctx, dirname: &PathValue, basename: &PathValue) -> Result<Vec<PathValue>> {
    if !is_empty(basename) {
        if lexists(ctx, &join_(dirname, basename)?)? {
            return Ok(vec![basename.clone()]);
        }
    } else if isdir(ctx, dirname)? {
        // `os.path.split()` returns an empty basename for paths ending with a
        // directory separator.  'q*x/' should match only directories.
        return Ok(vec![basename.clone()]);
    }
    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// `_rlistdir` / `_glob2`: recursive listing
// ---------------------------------------------------------------------------

/// One directory of a recursive listing.
struct Frame {
    dirname: PathValue,
    names: Vec<PathValue>,
    /// Index of the next name to look at; `names[next - 1]` is the current one.
    next: usize,
}

/// `_rlistdir(dirname, dir_fd, dironly, include_hidden)` as an explicit stack:
/// each name is yielded before its own directory is listed, and that listing
/// happens only when the consumer resumes.
struct Rlist {
    root: PathValue,
    dironly: bool,
    started: bool,
    /// The name just yielded may be a directory to descend into.
    descend: bool,
    stack: Vec<Frame>,
}

impl Rlist {
    fn new(root: PathValue, dironly: bool) -> Self {
        Self {
            root,
            dironly,
            started: false,
            descend: false,
            stack: Vec::new(),
        }
    }

    fn next(&mut self, ctx: &Ctx) -> Result<Option<PathValue>> {
        if !self.started {
            self.started = true;
            let names = listdir(ctx, &self.root, self.dironly)?;
            self.stack.push(Frame {
                dirname: self.root.clone(),
                names,
                next: 0,
            });
        } else if self.descend {
            self.descend = false;
            if let Some(top) = self.stack.last() {
                let name = &top.names[top.next - 1];
                // `path = _join(dirname, x) if dirname else x`
                let path = if is_empty(&top.dirname) {
                    name.clone()
                } else {
                    join_(&top.dirname, name)?
                };
                let names = listdir(ctx, &path, self.dironly)?;
                self.stack.push(Frame {
                    dirname: path,
                    names,
                    next: 0,
                });
            }
        }
        loop {
            let Some(top) = self.stack.last_mut() else {
                return Ok(None);
            };
            if top.next >= top.names.len() {
                self.stack.pop();
                continue;
            }
            top.next += 1;
            let name = &top.names[top.next - 1];
            if !(ctx.include_hidden || !ishidden(name)?) {
                continue;
            }
            self.descend = true;
            // `yield _join(x, y)` at every level above the one that listed it.
            let mut result = name.clone();
            for frame in self.stack[..self.stack.len() - 1].iter().rev() {
                result = join_(&frame.names[frame.next - 1], &result)?;
            }
            return Ok(Some(result));
        }
    }
}

/// `_glob2(dirname, pattern, dir_fd, dironly, include_hidden)`.
struct Glob2 {
    dirname: PathValue,
    /// `pattern[:0]`
    empty: PathValue,
    dironly: bool,
    rlist: Option<Rlist>,
}

impl Glob2 {
    fn new(dirname: PathValue, pattern: &PathValue, dironly: bool) -> Self {
        Self {
            dirname,
            empty: empty_like(pattern),
            dironly,
            rlist: None,
        }
    }

    fn next(&mut self, ctx: &Ctx) -> Result<Option<PathValue>> {
        if self.rlist.is_none() {
            self.rlist = Some(Rlist::new(self.dirname.clone(), self.dironly));
            if is_empty(&self.dirname) || isdir(ctx, &self.dirname)? {
                return Ok(Some(self.empty.clone()));
            }
        }
        match self.rlist.as_mut() {
            Some(rlist) => rlist.next(ctx),
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// `_iglob`
// ---------------------------------------------------------------------------

/// Where the names of one directory come from.
enum Source {
    Items(std::vec::IntoIter<PathValue>),
    Glob2(Box<Glob2>),
}

impl Source {
    fn items(items: Vec<PathValue>) -> Self {
        Self::Items(items.into_iter())
    }

    fn next(&mut self, ctx: &Ctx) -> Result<Option<PathValue>> {
        match self {
            Self::Items(items) => Ok(items.next()),
            Self::Glob2(glob2) => glob2.next(ctx),
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Glob0,
    Glob1,
    Glob2,
}

/// The directories a pattern's basename is matched in.
enum Dirs {
    Single(Option<PathValue>),
    /// `_iglob(dirname, ..., dironly=True)`.
    Nested(Box<IglobState>),
}

struct DirsStage {
    dirs: Dirs,
    mode: Mode,
    basename: PathValue,
    /// The directory being expanded and the names found in it.
    current: Option<(PathValue, Source)>,
}

enum Stage {
    Init,
    /// A single source whose names are yielded as they are.
    Direct(Source),
    Dirs(Box<DirsStage>),
    Done,
}

/// One `_iglob(pathname, root_dir, dir_fd, recursive, dironly, include_hidden)`
/// generator.
struct IglobState {
    pathname: PathValue,
    dironly: bool,
    stage: Stage,
}

impl IglobState {
    fn new(pathname: PathValue, dironly: bool) -> Self {
        Self {
            pathname,
            dironly,
            stage: Stage::Init,
        }
    }

    /// The part of `_iglob` before its first `yield`.
    fn init(&self, interp: &mut Interpreter, ctx: &Ctx) -> Result<Stage> {
        let (dirname, basename) = split_pv(&self.pathname);
        if !has_magic_pv(&self.pathname) {
            let found = if !is_empty(&basename) {
                lexists(ctx, &join_(&ctx.root_dir, &self.pathname)?)?
            } else {
                // Patterns ending with a slash should match only directories
                isdir(ctx, &join_(&ctx.root_dir, &dirname)?)?
            };
            let items = if found {
                vec![self.pathname.clone()]
            } else {
                Vec::new()
            };
            return Ok(Stage::Direct(Source::items(items)));
        }
        if is_empty(&dirname) {
            return Ok(Stage::Direct(if ctx.recursive && is_recursive(&basename) {
                Source::Glob2(Box::new(Glob2::new(
                    ctx.root_dir.clone(),
                    &basename,
                    self.dironly,
                )))
            } else {
                Source::items(glob1_in(
                    interp,
                    ctx,
                    &ctx.root_dir,
                    &basename,
                    self.dironly,
                )?)
            }));
        }
        // `os.path.split()` returns the argument itself as a dirname if it is a
        // drive or UNC path.  Prevent an infinite recursion if a drive or UNC
        // path contains magic characters (i.e. r'\\?\C:').
        let dirs = if !same_path(&dirname, &self.pathname) && has_magic_pv(&dirname) {
            Dirs::Nested(Box::new(IglobState::new(dirname, true)))
        } else {
            Dirs::Single(Some(dirname))
        };
        let mode = if has_magic_pv(&basename) {
            if ctx.recursive && is_recursive(&basename) {
                Mode::Glob2
            } else {
                Mode::Glob1
            }
        } else {
            Mode::Glob0
        };
        Ok(Stage::Dirs(Box::new(DirsStage {
            dirs,
            mode,
            basename,
            current: None,
        })))
    }

    fn next(&mut self, interp: &mut Interpreter, ctx: &Ctx) -> Result<Option<PathValue>> {
        loop {
            match &mut self.stage {
                Stage::Init => {
                    self.stage = Stage::Done;
                    self.stage = self.init(interp, ctx)?;
                }
                Stage::Done => return Ok(None),
                Stage::Direct(source) => {
                    let item = source.next(ctx);
                    if !matches!(item, Ok(Some(_))) {
                        self.stage = Stage::Done;
                    }
                    return item;
                }
                Stage::Dirs(stage) => {
                    if let Some((dirname, source)) = &mut stage.current {
                        if let Some(name) = source.next(ctx)? {
                            return Ok(Some(join_strict(dirname, &name)?));
                        }
                        stage.current = None;
                        continue;
                    }
                    let dirname = match &mut stage.dirs {
                        Dirs::Single(slot) => slot.take(),
                        Dirs::Nested(nested) => nested.next(interp, ctx)?,
                    };
                    let Some(dirname) = dirname else {
                        self.stage = Stage::Done;
                        continue;
                    };
                    let joined = join_(&ctx.root_dir, &dirname)?;
                    let source = match stage.mode {
                        Mode::Glob0 => Source::items(glob0_in(ctx, &joined, &stage.basename)?),
                        Mode::Glob1 => Source::items(glob1_in(
                            interp,
                            ctx,
                            &joined,
                            &stage.basename,
                            self.dironly,
                        )?),
                        Mode::Glob2 => Source::Glob2(Box::new(Glob2::new(
                            joined,
                            &stage.basename,
                            self.dironly,
                        ))),
                    };
                    stage.current = Some((dirname, source));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// `iglob()` / `glob()`
// ---------------------------------------------------------------------------

/// The iterator `iglob()` returns.
struct GlobState {
    ctx: Ctx,
    root: Option<IglobState>,
    /// The first name, already produced by `iglob()`'s eager "skip the empty
    /// string" step.
    first: Option<PathValue>,
    /// `pathname` was neither `str` nor `bytes`: the type named by the
    /// `TypeError` `os.path.split(pathname)` raises on the first `next()`.
    bad_type: Option<String>,
    done: bool,
}

impl GlobState {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<PathValue>> {
        if let Some(first) = self.first.take() {
            return Ok(Some(first));
        }
        if self.done {
            return Ok(None);
        }
        let result = if let Some(type_name) = self.bad_type.take() {
            Err(type_error(format!(
                "expected str, bytes or os.PathLike object, not {type_name}"
            )))
        } else if let Some(root) = self.root.as_mut() {
            root.next(interp, &self.ctx)
        } else {
            Ok(None)
        };
        if !matches!(result, Ok(Some(_))) {
            // An exhausted generator and one that raised are both finished.
            self.done = true;
            self.root = None;
        }
        result
    }
}

/// `sys.audit(event, *args)`.
fn audit(interp: &mut Interpreter, event: &str, args: Vec<Value>) -> Result<()> {
    let hooks: Vec<Value> = interp
        .features()
        .sys_with(|state| state.audit_hooks.clone());
    if hooks.is_empty() {
        return Ok(());
    }
    let call_args = [
        ExpandedCallArg {
            name: None,
            value: Value::string(event),
        },
        ExpandedCallArg {
            name: None,
            value: Value::tuple(args),
        },
    ];
    for hook in hooks {
        interp.call_function_expanded(hook, &call_args)?;
    }
    Ok(())
}

/// The body of `iglob()` up to (not including) returning the iterator.
fn start_iglob(interp: &mut Interpreter, sig: &Sig, args: &[ExpandedCallArg]) -> Result<GlobState> {
    let mut bound = args::bind(sig, args)?;
    let pathname = bound.take_or_none(0);
    let root_dir = bound.take_or_none(1);
    let dir_fd = bound.take_or_none(2);
    let recursive = bound.take_or(3, || Value::bool_(false));
    let include_hidden = bound.take_or(4, || Value::bool_(false));

    audit(
        interp,
        "glob.glob",
        vec![pathname.clone(), recursive.clone()],
    )?;
    audit(
        interp,
        "glob.glob/2",
        vec![
            pathname.clone(),
            recursive.clone(),
            root_dir.clone(),
            dir_fd.clone(),
        ],
    )?;

    let pathname_pv = extract_str_or_bytes(&pathname);
    let root_dir_pv = if root_dir.is_none() {
        // `pathname[:0]`
        match &pathname_pv {
            Some(pv) => empty_like(pv),
            None => {
                let slice = crate::interpreter::make_slice_value(None, Some(Value::int(0)), None);
                let sliced = crate::builtin_modules::operator::getitem_value(
                    interp,
                    pathname.clone(),
                    slice,
                )?;
                extract_str_or_bytes(&sliced).unwrap_or_else(|| PathValue::Str(String::new()))
            }
        }
    } else {
        fspath_resolve(interp, &root_dir)?
    };
    let recursive = interp.truthy_value(&recursive)?;
    let include_hidden = interp.truthy_value(&include_hidden)?;
    let ctx = Ctx {
        root_dir: root_dir_pv,
        recursive,
        include_hidden,
        dir_fd: !dir_fd.is_none(),
    };

    let Some(pathname_pv) = pathname_pv else {
        // `os.path.split()` rejects anything but `str` / `bytes` / `PathLike`;
        // an empty one is looked at (and so raises) right away.
        let mut state = GlobState {
            ctx,
            root: None,
            first: None,
            bad_type: Some(value_type_name_str(&pathname).to_string()),
            done: false,
        };
        if !interp.truthy_value(&pathname)? {
            state.step(interp)?;
        }
        return Ok(state);
    };

    let skip_empty = is_empty(&pathname_pv)
        || (ctx.recursive
            && match &pathname_pv {
                PathValue::Str(s) => s.starts_with("**"),
                PathValue::Bytes(b) => b.starts_with(b"**"),
            });
    let mut state = GlobState {
        ctx,
        root: Some(IglobState::new(pathname_pv, false)),
        first: None,
        bad_type: None,
        done: false,
    };
    if skip_empty {
        // `s = next(it)  # skip empty string`
        if let Some(first) = state.step(interp)?
            && !is_empty(&first)
        {
            state.first = Some(first);
        }
    }
    Ok(state)
}

fn glob_state_error() -> PyError {
    PyError::runtime("internal: glob iterator state corrupted".to_string())
}

fn advance_iglob(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    // The state is moved out while a step runs, so a re-entrant `next()`
    // reports what a running generator would.
    let taken = state_rc
        .borrow_mut()
        .downcast_mut::<Option<GlobState>>()
        .map(Option::take);
    let mut state = match taken {
        Some(Some(state)) => state,
        Some(None) => {
            return Err(PyError::named(
                "ValueError",
                "generator already executing".to_string(),
            ));
        }
        None => return Err(glob_state_error()),
    };
    let result = state.step(interp);
    if let Some(slot) = state_rc.borrow_mut().downcast_mut::<Option<GlobState>>() {
        *slot = Some(state);
    }
    result.map(|item| item.map(PathValue::into_value))
}

// ---------------------------------------------------------------------------
// has_magic() / escape()
// ---------------------------------------------------------------------------

fn has_magic_value(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if let Some(pv) = extract_str_or_bytes(value) {
        return Ok(has_magic_pv(&pv));
    }
    if PyBytesLike::matches(value) {
        return Err(type_error(
            "cannot use a string pattern on a bytes-like object",
        ));
    }
    let _ = interp;
    Err(type_error(format!(
        "expected string or bytes-like object, got '{}'",
        value_type_name_str(value)
    )))
}

/// `os.path.splitdrive(path)`.
fn splitdrive_pv(path: PathValue) -> (PathValue, PathValue) {
    #[cfg(windows)]
    {
        match path {
            PathValue::Str(s) => {
                let (drive, tail) = host_splitdrive_str(&s);
                (PathValue::Str(drive), PathValue::Str(tail))
            }
            PathValue::Bytes(b) => {
                let (drive, tail) = host_splitdrive_bytes(&b);
                (PathValue::Bytes(drive), PathValue::Bytes(tail))
            }
        }
    }
    #[cfg(not(windows))]
    {
        (empty_like(&path), path)
    }
}

/// `magic_check.sub(r'[\1]', s)`: wrap every `*`, `?` and `[` in brackets.
fn escape_pv(tail: PathValue) -> PathValue {
    match tail {
        PathValue::Str(s) => {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                if matches!(c, '*' | '?' | '[') {
                    out.push('[');
                    out.push(c);
                    out.push(']');
                } else {
                    out.push(c);
                }
            }
            PathValue::Str(out)
        }
        PathValue::Bytes(b) => {
            let mut out = Vec::with_capacity(b.len());
            for c in b {
                if matches!(c, b'*' | b'?' | b'[') {
                    out.extend_from_slice(&[b'[', c, b']']);
                } else {
                    out.push(c);
                }
            }
            PathValue::Bytes(out)
        }
    }
}

// ---------------------------------------------------------------------------
// `glob0()` / `glob1()` operands
// ---------------------------------------------------------------------------

fn operand(interp: &mut Interpreter, value: &Value) -> Result<PathValue> {
    fspath_resolve(interp, value)
}

fn paths_to_list(items: Vec<PathValue>) -> Value {
    Value::list(items.into_iter().map(PathValue::into_value).collect())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
    }

    // Return a list of paths matching a pathname pattern.
    //
    // The pattern may contain simple shell-style wildcards a la
    // fnmatch. Unlike fnmatch, filenames starting with a
    // dot are special cases that are not matched by '*' and '?'
    // patterns by default.
    //
    // If `include_hidden` is true, the patterns '*', '?', '**'  will match hidden
    // directories.
    //
    // If `recursive` is true, the pattern '**' will match any files and
    // zero or more directories and subdirectories.
    fn glob(args) -> Result<Value> {
        let mut state = start_iglob(_interp, &SIG_GLOB, args)?;
        let mut items = Vec::new();
        while let Some(item) = state.step(_interp)? {
            items.push(item);
        }
        Ok(paths_to_list(items))
    }

    // Return an iterator which yields the paths matching a pathname pattern.
    //
    // The pattern may contain simple shell-style wildcards a la
    // fnmatch. However, unlike fnmatch, filenames starting with a
    // dot are special cases that are not matched by '*' and '?'
    // patterns.
    //
    // If recursive is true, the pattern '**' will match any files and
    // zero or more directories and subdirectories.
    fn iglob(args) -> Result<Value> {
        let state = start_iglob(_interp, &SIG_IGLOB, args)?;
        Ok(Value::generator(Box::new(ProviderIterator::new(
            Box::new(Some(state)),
            advance_iglob,
            "generator",
            "generator",
            None,
        ))))
    }

    // Not public but used by third-party code.
    fn glob0(args) -> Result<Value> {
        let mut b = args::bind(&SIG_GLOB0, args)?;
        let dirname = b.take_or_none(0);
        let pattern = b.take_or_none(1);
        let dirname = operand(_interp, &dirname)?;
        let pattern = operand(_interp, &pattern)?;
        Ok(paths_to_list(glob0_in(&Ctx::bare(empty_like(&dirname)), &dirname, &pattern)?))
    }

    // Not public but used by third-party code.
    fn glob1(args) -> Result<Value> {
        let mut b = args::bind(&SIG_GLOB1, args)?;
        let dirname = b.take_or_none(0);
        let pattern = b.take_or_none(1);
        let dirname = operand(_interp, &dirname)?;
        let pattern = operand(_interp, &pattern)?;
        let ctx = Ctx::bare(empty_like(&dirname));
        Ok(paths_to_list(glob1_in(_interp, &ctx, &dirname, &pattern, false)?))
    }

    fn has_magic(args) -> Result<Value> {
        let mut b = args::bind(&SIG_HAS_MAGIC, args)?;
        let s = b.take_or_none(0);
        Ok(Value::bool_(has_magic_value(_interp, &s)?))
    }

    // Escape all special characters.
    fn escape(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ESCAPE, args)?;
        let pathname = b.take_or_none(0);
        // Escaping is done by wrapping any of "*?[" between square brackets.
        // Metacharacters do not work in the drive part and shouldn't be escaped.
        let (drive, tail) = splitdrive_pv(fspath_resolve(_interp, &pathname)?);
        let escaped = escape_pv(tail);
        Ok(match (drive, escaped) {
            (PathValue::Str(mut drive), PathValue::Str(tail)) => {
                drive.push_str(&tail);
                Value::string(drive)
            }
            (PathValue::Bytes(mut drive), PathValue::Bytes(tail)) => {
                drive.extend_from_slice(&tail);
                Value::bytes(drive)
            }
            // `splitdrive` keeps the operand's type.
            (drive, _) => drive.into_value(),
        })
    }
}
