// `platform` module — 100% native Rust port of CPython 3.12.3's
// `Lib/platform.py` (issues #3209, #3469); there is no Python-source
// injection.
//
// The public API (`uname`, `system`, `node`, `release`, `version`, `machine`,
// `processor`, `architecture`, `platform`, `system_alias`, `win32_ver`,
// `win32_edition`, `win32_is_iot`, `mac_ver`, `java_ver`, `libc_ver`,
// `python_*`, `freedesktop_os_release`) and the private helpers the CPython
// test-suite and third-party code reach for (`_platform`, `_sys_version`,
// `_parse_os_release`, `_comparable_version`, `_norm_version`, `_syscmd_ver`,
// `_node`, `_win32_ver`, `_mac_ver_xml`, `_unknown_as_blank`) keep CPython's
// signatures and results.
//
// Like the CPython source, the module keeps its state in ordinary module
// globals (`_uname_cache`, `_platform_cache`, `_sys_version_cache`,
// `_os_release_cache`), and every function that calls another public function
// of the module looks it up on the live module (`uname()`, `system_alias()`,
// `win32_ver()`, `mac_ver()`, `libc_ver()`, `java_ver()`, `architecture()`,
// `_platform()`, `_sys_version()`, ...), so monkeypatching `platform.X` and
// clearing the caches behave as on CPython.
//
// `uname_result` is a real subclass of a `collections.namedtuple` with five
// fields whose sixth field, `processor`, is a `functools.cached_property`;
// `post_load_init` builds that base and wires it up.
//
// Where CPython shells out, this port uses native probes instead:
// `os.uname()` is `libc::uname`, the Windows version comes from
// `RtlGetVersion` (what the WMI `Win32_OperatingSystem` query reports),
// `libc_ver()` asks glibc directly, and `architecture()` reads the
// executable's ELF / Mach-O / PE header instead of running `file -b`.
//
// What this port deliberately does not do:
// - `re` is not imported: the regular expressions of `libc_ver`,
//   `_sys_version`, `_syscmd_ver`, `_comparable_version` and
//   `_parse_os_release` are matched by hand, so `platform.re` does not exist.
// - `_wmi_query`, `_Processor`, `_syscmd_file` and `_follow_symlinks` are not
//   exposed (their jobs are done by the native probes above).
// - `java_ver()` always reports the defaults: there is no `java.lang`.
// - `sys.version` of this runtime is not in CPython's
//   `"3.12.3 (main, ...) [compiler]"` shape, so `python_*()` fall back to
//   values derived from `sys.implementation` / `sys.version_info` when the
//   default `sys.version` cannot be parsed; an explicit, unparsable
//   `_sys_version(sys_version)` raises `ValueError` as on CPython.
//
// Reference: <https://docs.python.org/3.12/library/platform.html>

use std::cell::RefCell;
use std::io::Read;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::genericpath::ops::{PathValue, fspath_resolve};
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, class_is_subclass_of, lookup_exc_class,
    make_iterator, render_value_repr,
};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const MODULE_DOC: &str = " This module tries to retrieve as much platform-identifying data as\n    \
possible. It makes this information available via function APIs.\n\n    \
If called from the command line, it prints the platform\n    \
information concatenated as single string to stdout. The output\n    \
format is usable as part of a filename.\n\n";

const COPYRIGHT: &str = "\n    \
Copyright (c) 1999-2000, Marc-Andre Lemburg; mailto:mal@lemburg.com\n    \
Copyright (c) 2000-2010, eGenix.com Software GmbH; mailto:info@egenix.com\n\n    \
Permission to use, copy, modify, and distribute this software and its\n    \
documentation for any purpose and without fee or royalty is hereby granted,\n    \
provided that the above copyright notice appear in all copies and that\n    \
both that copyright notice and this permission notice appear in\n    \
supporting documentation or portions thereof, including modifications,\n    \
that you make.\n\n    \
EGENIX.COM SOFTWARE GMBH DISCLAIMS ALL WARRANTIES WITH REGARD TO\n    \
THIS SOFTWARE, INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND\n    \
FITNESS, IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL,\n    \
INDIRECT OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING\n    \
FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,\n    \
NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION\n    \
WITH THE USE OR PERFORMANCE OF THIS SOFTWARE !\n\n";

const UNAME_RESULT_DOC: &str = "\n    \
A uname_result that's largely compatible with a\n    \
simple namedtuple except that 'processor' is\n    \
resolved late and cached to avoid calling \"uname\"\n    \
except when needed.\n    ";

/// Module namespace order of CPython 3.12.3's `platform.py` (after the dunder
/// entries); `post_load_init` restores it.
const MODULE_ORDER: &[&str] = &[
    "__version__",
    "collections",
    "os",
    "sys",
    "functools",
    "itertools",
    "_ver_stages",
    "_comparable_version",
    "libc_ver",
    "_norm_version",
    "_syscmd_ver",
    "_WIN32_CLIENT_RELEASES",
    "_WIN32_SERVER_RELEASES",
    "win32_is_iot",
    "win32_edition",
    "_win32_ver",
    "win32_ver",
    "_mac_ver_xml",
    "mac_ver",
    "java_ver",
    "system_alias",
    "_platform",
    "_node",
    "_default_architecture",
    "architecture",
    "_unknown_as_blank",
    "uname_result",
    "_uname_cache",
    "uname",
    "system",
    "node",
    "release",
    "version",
    "machine",
    "processor",
    "_sys_version_cache",
    "_sys_version",
    "python_implementation",
    "python_version",
    "python_version_tuple",
    "python_branch",
    "python_revision",
    "python_build",
    "python_compiler",
    "_platform_cache",
    "platform",
    "_os_release_candidates",
    "_os_release_cache",
    "_parse_os_release",
    "freedesktop_os_release",
];

const UNAME_FIELDS: [&str; 6] = [
    "system",
    "node",
    "release",
    "version",
    "machine",
    "processor",
];

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

const SIG_UNKNOWN_AS_BLANK: Sig = sig("_unknown_as_blank", &["val"], 0);
const SIG_COMPARABLE_VERSION: Sig = sig("_comparable_version", &["version"], 0);
const SIG_LIBC_VER: Sig = sig(
    "libc_ver",
    &["executable", "lib", "version", "chunksize"],
    4,
);
const SIG_NORM_VERSION: Sig = sig("_norm_version", &["version", "build"], 1);
const SIG_SYSCMD_VER: Sig = sig(
    "_syscmd_ver",
    &["system", "release", "version", "supported_platforms"],
    4,
);
const SIG_WIN32_VER: Sig = sig("win32_ver", &["release", "version", "csd", "ptype"], 4);
const SIG_WIN32_VER_PRIVATE: Sig = sig("_win32_ver", &["version", "csd", "ptype"], 0);
const SIG_MAC_VER: Sig = sig("mac_ver", &["release", "versioninfo", "machine"], 3);
const SIG_JAVA_VER: Sig = sig("java_ver", &["release", "vendor", "vminfo", "osinfo"], 4);
const SIG_SYSTEM_ALIAS: Sig = sig("system_alias", &["system", "release", "version"], 0);
const SIG_NODE: Sig = sig("_node", &["default"], 1);
const SIG_ARCHITECTURE: Sig = sig("architecture", &["executable", "bits", "linkage"], 3);
const SIG_SYS_VERSION: Sig = sig("_sys_version", &["sys_version"], 1);
const SIG_PLATFORM: Sig = sig("platform", &["aliased", "terse"], 2);
const SIG_PARSE_OS_RELEASE: Sig = sig("_parse_os_release", &["lines"], 0);
const SIG_MAKE: Sig = sig("uname_result._make", &["cls", "iterable"], 0);
const SIG_SELF_ONLY: Sig = sig("uname_result.method", &["self"], 0);
const SIG_GETITEM: Sig = sig("uname_result.__getitem__", &["self", "key"], 0);

// ── Calling helpers ──────────────────────────────────────────────────────────

fn call(interp: &mut Interpreter, callee: &Value, values: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = values
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee.clone(), &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: &Value,
    values: &[Value],
    keywords: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = values
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(keywords.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee.clone(), &expanded)
}

/// `receiver.name(*values)`.
fn method(
    interp: &mut Interpreter,
    receiver: &Value,
    name: &str,
    values: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(receiver, name)?;
    call(interp, &callee, values)
}

fn module_value(interp: &mut Interpreter) -> Result<Value> {
    interp.load_module("platform")
}

/// A global of the live `platform` module (what the CPython source reads as a
/// bare name, so monkeypatching `platform.X` is honoured).
fn global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = module_value(interp)?;
    interp.get_attr(&module, name)
}

fn set_global(interp: &mut Interpreter, name: &str, value: Value) -> Result<()> {
    let module = module_value(interp)?;
    interp.assign_attr(module, name, value)
}

fn call_global(interp: &mut Interpreter, name: &str, values: &[Value]) -> Result<Value> {
    let function = global(interp, name)?;
    call(interp, &function, values)
}

fn sys_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let sys = interp.load_module("sys")?;
    interp.get_attr(&sys, name)
}

fn sys_platform(interp: &mut Interpreter) -> Result<String> {
    let value = sys_attr(interp, "platform")?;
    Ok(value.as_str().unwrap_or_default().to_string())
}

fn binary(interp: &mut Interpreter, left: &Value, op: BinaryOp, right: &Value) -> Result<bool> {
    let result = interp.eval_binary(left.clone(), op, right.clone())?;
    interp.truthy_value(&result)
}

fn py_eq(interp: &mut Interpreter, left: &Value, right: &Value) -> Result<bool> {
    binary(interp, left, BinaryOp::Eq, right)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

fn streq(value: &Value, text: &str) -> bool {
    value.as_str() == Some(text)
}

/// `def name():` — reject any argument with CPython's wording.
fn no_args(fn_name: &'static str, args: &[ExpandedCallArg]) -> Result<()> {
    let name = fn_name.rsplit('.').next().unwrap_or(fn_name);
    args::bind(&sig(name, &[], 0), args).map(|_| ())
}

fn s(text: &str) -> Value {
    Value::string(text)
}

fn type_error(message: impl Into<String>) -> PyError {
    PyError::named("TypeError", message.into())
}

fn value_error(message: impl Into<String>) -> PyError {
    PyError::named("ValueError", message.into())
}

/// `a, b, ... = items` for exactly `n` targets.
fn unpack(items: Vec<Value>, n: usize) -> Result<Vec<Value>> {
    match items.len().cmp(&n) {
        std::cmp::Ordering::Equal => Ok(items),
        std::cmp::Ordering::Greater => Err(value_error(format!(
            "too many values to unpack (expected {n})"
        ))),
        std::cmp::Ordering::Less => Err(value_error(format!(
            "not enough values to unpack (expected {n}, got {})",
            items.len()
        ))),
    }
}

fn unpack_iterable(interp: &mut Interpreter, value: &Value, n: usize) -> Result<Vec<Value>> {
    let items = interp.collect_iterable(value)?;
    unpack(items, n)
}

/// Whether `error` is an instance of the named builtin exception class
/// (`except OSError:`), plus an equivalent error to re-raise.
fn error_is(
    interp: &mut Interpreter,
    error: PyError,
    name: &str,
) -> Result<(Option<Value>, PyError)> {
    let exception = interp.materialize_pyerror(error)?;
    let hit = exception.as_py_instance_rc().is_some_and(|instance| {
        let class = Rc::clone(&instance.borrow().class);
        lookup_exc_class(name).is_some_and(|expected| class_is_subclass_of(&class, &expected))
    });
    Ok((hit.then(|| exception.clone()), PyError::raised(exception)))
}

fn new_dict() -> Value {
    Value::dict(PyDict::default())
}

fn dict_set(dict: &Value, key: &str, value: Value) -> Result<()> {
    dict.dict_insert(PyKey::str_from(key), value)?;
    Ok(())
}

// ── Python `int()` of a string ───────────────────────────────────────────────

/// `int(text)` for the decimal text forms CPython accepts: surrounding
/// whitespace, an optional sign and digits with single underscores between
/// them.
fn parse_py_int(text: &str) -> Option<i128> {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c));
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') {
        return None;
    }
    let mut value: i128 = 0;
    let mut previous_underscore = false;
    for byte in digits.bytes() {
        if byte == b'_' {
            if previous_underscore {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        previous_underscore = false;
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(i128::from(byte - b'0'))?;
    }
    Some(if negative { -value } else { value })
}

fn int_value(number: i128) -> Value {
    match i64::try_from(number) {
        Ok(small) => Value::int(small),
        Err(_) => Value::string(number.to_string()),
    }
}

/// `int(text)`, raising CPython's `ValueError` on malformed input.
fn py_int(interp: &mut Interpreter, text: &str) -> Result<i128> {
    parse_py_int(text).ok_or_else(|| {
        let repr =
            render_value_repr(interp, &Value::string(text)).unwrap_or_else(|_| format!("'{text}'"));
        value_error(format!("invalid literal for int() with base 10: {repr}"))
    })
}

// ── Greedy backtracking matcher (the regular expressions of the module) ──────

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn is_ascii_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `[class]{min,}` followed by the continuation `k`: take the longest run,
/// then give characters back one at a time until `k` accepts the position.
fn greedy(
    text: &[u8],
    from: usize,
    min: usize,
    accepts: &dyn Fn(u8) -> bool,
    k: &mut dyn FnMut(usize) -> bool,
) -> bool {
    let mut end = from;
    while end < text.len() && accepts(text[end]) {
        end += 1;
    }
    let mut at = end;
    loop {
        if at < from + min {
            return false;
        }
        if k(at) {
            return true;
        }
        if at == from {
            return false;
        }
        at -= 1;
    }
}

type Span = Option<(usize, usize)>;

/// CPython's `sys_version_parser`:
/// `([\w.+]+)\s*\(#?([^,]+)(?:,\s*([\w ]*)(?:,\s*([\w :]*))?)?\)\s*\[([^\]]+)\]?`
/// (`re.ASCII`, anchored at the start).  Returns the five group spans.
struct CPythonVersionParser<'a> {
    text: &'a [u8],
    groups: RefCell<[Span; 5]>,
}

impl CPythonVersionParser<'_> {
    fn run(&self) -> bool {
        let text = self.text;
        greedy(
            text,
            0,
            1,
            &|b| is_ascii_word(b) || b == b'.' || b == b'+',
            &mut |end1| {
                self.groups.borrow_mut()[0] = Some((0, end1));
                greedy(text, end1, 0, &is_space, &mut |p| {
                    if text.get(p) != Some(&b'(') {
                        return false;
                    }
                    let after = p + 1;
                    if text.get(after) == Some(&b'#') && self.build(after + 1) {
                        return true;
                    }
                    self.build(after)
                })
            },
        )
    }

    fn build(&self, from: usize) -> bool {
        greedy(self.text, from, 1, &|b| b != b',', &mut |end2| {
            self.groups.borrow_mut()[1] = Some((from, end2));
            self.optional_date(end2)
        })
    }

    fn optional_date(&self, at: usize) -> bool {
        let text = self.text;
        if text.get(at) == Some(&b',') {
            let taken = greedy(text, at + 1, 0, &is_space, &mut |q| {
                greedy(
                    text,
                    q,
                    0,
                    &|b| is_ascii_word(b) || b == b' ',
                    &mut |end3| {
                        self.groups.borrow_mut()[2] = Some((q, end3));
                        self.groups.borrow_mut()[3] = None;
                        if self.optional_time(end3) {
                            return true;
                        }
                        self.groups.borrow_mut()[3] = None;
                        self.close(end3)
                    },
                )
            });
            if taken {
                return true;
            }
        }
        self.groups.borrow_mut()[2] = None;
        self.groups.borrow_mut()[3] = None;
        self.close(at)
    }

    fn optional_time(&self, at: usize) -> bool {
        let text = self.text;
        if text.get(at) != Some(&b',') {
            return false;
        }
        greedy(text, at + 1, 0, &is_space, &mut |q| {
            greedy(
                text,
                q,
                0,
                &|b| is_ascii_word(b) || b == b' ' || b == b':',
                &mut |end4| {
                    self.groups.borrow_mut()[3] = Some((q, end4));
                    self.close(end4)
                },
            )
        })
    }

    fn close(&self, at: usize) -> bool {
        let text = self.text;
        if text.get(at) != Some(&b')') {
            return false;
        }
        greedy(text, at + 1, 0, &is_space, &mut |p| {
            if text.get(p) != Some(&b'[') {
                return false;
            }
            greedy(text, p + 1, 1, &|b| b != b']', &mut |end5| {
                self.groups.borrow_mut()[4] = Some((p + 1, end5));
                true
            })
        })
    }
}

fn match_cpython_version(text: &[u8]) -> Option<[Span; 5]> {
    let parser = CPythonVersionParser {
        text,
        groups: RefCell::new([None; 5]),
    };
    parser.run().then(|| parser.groups.into_inner())
}

/// CPython's `pypy_sys_version_parser`:
/// `([\w.+]+)\s*\(#?([^,]+),\s*([\w ]+),\s*([\w :]+)\)\s*\[PyPy [^\]]+\]?`
/// (default flags, anchored at the start).  Returns the four group spans.
fn match_pypy_version(text: &[u8]) -> Option<[Span; 4]> {
    let groups: RefCell<[Span; 4]> = RefCell::new([None; 4]);
    let is_str_word = |b: u8| is_word(b);
    let found = greedy(
        text,
        0,
        1,
        &|b| is_str_word(b) || b == b'.' || b == b'+',
        &mut |end1| {
            groups.borrow_mut()[0] = Some((0, end1));
            greedy(text, end1, 0, &is_space, &mut |p| {
                if text.get(p) != Some(&b'(') {
                    return false;
                }
                let tail = |from: usize| -> bool {
                    greedy(text, from, 1, &|b| b != b',', &mut |end2| {
                        groups.borrow_mut()[1] = Some((from, end2));
                        if text.get(end2) != Some(&b',') {
                            return false;
                        }
                        greedy(text, end2 + 1, 0, &is_space, &mut |q| {
                            greedy(text, q, 1, &|b| is_str_word(b) || b == b' ', &mut |end3| {
                                groups.borrow_mut()[2] = Some((q, end3));
                                if text.get(end3) != Some(&b',') {
                                    return false;
                                }
                                greedy(text, end3 + 1, 0, &is_space, &mut |r| {
                                    greedy(
                                        text,
                                        r,
                                        1,
                                        &|b| is_str_word(b) || b == b' ' || b == b':',
                                        &mut |end4| {
                                            groups.borrow_mut()[3] = Some((r, end4));
                                            if text.get(end4) != Some(&b')') {
                                                return false;
                                            }
                                            greedy(text, end4 + 1, 0, &is_space, &mut |t| {
                                                text[t..].starts_with(b"[PyPy ")
                                                    && greedy(
                                                        text,
                                                        t + 6,
                                                        1,
                                                        &|b| b != b']',
                                                        &mut |_| true,
                                                    )
                                            })
                                        },
                                    )
                                })
                            })
                        })
                    })
                };
                if text.get(p + 1) == Some(&b'#') && tail(p + 2) {
                    return true;
                }
                tail(p + 1)
            })
        },
    );
    found.then(|| groups.into_inner())
}

fn slice_text(text: &[u8], span: Span) -> Option<String> {
    span.map(|(start, end)| String::from_utf8_lossy(&text[start..end]).into_owned())
}

// ── Version comparison (`_comparable_version`) ───────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionPart {
    /// A digit run, kept as its decimal digits without leading zeros.
    Number(String),
    Text(String),
}

fn version_stage(name: &str) -> i64 {
    match name {
        "dev" => 10,
        "alpha" | "a" => 20,
        "beta" | "b" => 30,
        "c" => 40,
        "RC" | "rc" => 50,
        "pl" | "p" => 200,
        _ => 0,
    }
}

/// `_comparable_version`: the list of `(stage, part)` pairs of a version.
fn comparable_version(version: &str) -> Vec<(i64, VersionPart)> {
    let mut result = Vec::new();
    let mut chunk = String::new();
    let flush = |chunk: &mut String, result: &mut Vec<(i64, VersionPart)>| {
        if !chunk.is_empty() {
            result.push((
                version_stage(chunk),
                VersionPart::Text(std::mem::take(chunk)),
            ));
        }
    };
    let mut chars = version.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            flush(&mut chunk, &mut result);
            let mut digits = String::from(c);
            while let Some(&next) = chars.peek() {
                if !next.is_ascii_digit() {
                    break;
                }
                digits.push(next);
                chars.next();
            }
            let trimmed = digits.trim_start_matches('0');
            let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
            result.push((100, VersionPart::Number(trimmed.to_string())));
        } else if matches!(c, '.' | '_' | '+' | '-') {
            flush(&mut chunk, &mut result);
        } else {
            chunk.push(c);
        }
    }
    flush(&mut chunk, &mut result);
    result
}

fn compare_parts(left: &VersionPart, right: &VersionPart) -> std::cmp::Ordering {
    match (left, right) {
        (VersionPart::Number(a), VersionPart::Number(b)) => {
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        }
        (VersionPart::Text(a), VersionPart::Text(b)) => a.cmp(b),
        (VersionPart::Number(_), VersionPart::Text(_)) => std::cmp::Ordering::Less,
        (VersionPart::Text(_), VersionPart::Number(_)) => std::cmp::Ordering::Greater,
    }
}

/// `V(left) > V(right)` on `_comparable_version` lists.
fn version_greater(left: &str, right: &str) -> bool {
    let left = comparable_version(left);
    let right = comparable_version(right);
    for (a, b) in left.iter().zip(right.iter()) {
        let order = a.0.cmp(&b.0).then_with(|| compare_parts(&a.1, &b.1));
        if order != std::cmp::Ordering::Equal {
            return order == std::cmp::Ordering::Greater;
        }
    }
    left.len() > right.len()
}

// ── OS probes ────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod win32_info {
    #[repr(C)]
    struct OsVersionInfoExW {
        dw_os_version_info_size: u32,
        dw_major_version: u32,
        dw_minor_version: u32,
        dw_build_number: u32,
        dw_platform_id: u32,
        sz_csd_version: [u16; 128],
        w_service_pack_major: u16,
        w_service_pack_minor: u16,
        w_suite_mask: u16,
        w_product_type: u8,
        w_reserved: u8,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(lp_version_information: *mut OsVersionInfoExW) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetComputerNameExW(name_type: i32, buffer: *mut u16, size: *mut u32) -> i32;
    }

    /// `(version, csd, ptype, is_client)` as the WMI `Win32_OperatingSystem`
    /// query of `platform._win32_ver` reports them.
    pub(super) fn query_win32_ver() -> Option<(String, String, String, bool)> {
        let mut info = OsVersionInfoExW {
            dw_os_version_info_size: std::mem::size_of::<OsVersionInfoExW>() as u32,
            dw_major_version: 0,
            dw_minor_version: 0,
            dw_build_number: 0,
            dw_platform_id: 0,
            sz_csd_version: [0; 128],
            w_service_pack_major: 0,
            w_service_pack_minor: 0,
            w_suite_mask: 0,
            w_product_type: 0,
            w_reserved: 0,
        };
        // SAFETY: `info` is properly sized with `dw_os_version_info_size` set to
        // `size_of::<OsVersionInfoExW>()` as required by `RtlGetVersion`.
        let status = unsafe { RtlGetVersion(&mut info) };
        if status != 0 {
            return None;
        }
        let version = format!(
            "{}.{}.{}",
            info.dw_major_version, info.dw_minor_version, info.dw_build_number
        );
        let csd = if info.w_service_pack_minor != 0 {
            format!(
                "SP{}.{}",
                info.w_service_pack_major, info.w_service_pack_minor
            )
        } else {
            format!("SP{}", info.w_service_pack_major)
        };
        Some((
            version,
            csd,
            "Multiprocessor Free".to_string(),
            info.w_product_type == 1,
        ))
    }

    /// `socket.gethostname()`: the DNS host name of the computer.
    pub(super) fn dns_host_name() -> Option<String> {
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        // SAFETY: `buffer` holds `size` UTF-16 units; `GetComputerNameExW`
        // writes at most that many and updates `size` to the length written.
        let ok = unsafe { GetComputerNameExW(1, buffer.as_mut_ptr(), &mut size) };
        if ok == 0 {
            return std::env::var("COMPUTERNAME").ok();
        }
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }

    pub(super) fn machine_fallback() -> String {
        match std::env::consts::ARCH {
            "x86_64" => "AMD64".to_string(),
            "aarch64" => "ARM64".to_string(),
            "x86" => "x86".to_string(),
            other => other.to_string(),
        }
    }
}

/// `os.uname()` as `(sysname, nodename, release, version, machine)`, `None`
/// where the platform has no `os.uname` (Windows).
fn host_os_uname() -> Option<[String; 5]> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: `libc::utsname` is a plain C struct of fixed byte arrays and
        // `libc::uname` populates all five fields on success.
        let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::uname(&mut uts) };
        if rc != 0 {
            return None;
        }
        let to_str = |buf: &[libc::c_char]| -> String {
            // SAFETY: `uname` NUL-terminates each field on success.
            unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        };
        Some([
            to_str(&uts.sysname),
            to_str(&uts.nodename),
            to_str(&uts.release),
            to_str(&uts.version),
            to_str(&uts.machine),
        ])
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `socket.gethostname()` (what `platform._node` returns).
fn host_node() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: `buf` is a writable buffer of the stated length.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }
    #[cfg(windows)]
    {
        win32_info::dns_host_name()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// `_get_machine_win32()`: the `PROCESSOR_*` environment variables (WOW64
/// processes mask the native architecture).
fn host_machine_win32() -> String {
    let from_env = std::env::var("PROCESSOR_ARCHITEW6432")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("PROCESSOR_ARCHITECTURE")
                .ok()
                .filter(|value| !value.is_empty())
        });
    if let Some(value) = from_env {
        return value;
    }
    #[cfg(windows)]
    {
        win32_info::machine_fallback()
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

/// `_Processor.get()`.
fn host_processor(platform_name: &str) -> String {
    if platform_name == "win32" {
        return std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| host_machine_win32());
    }
    // `uname -p`.
    let output = std::process::Command::new("uname")
        .arg("-p")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// `os.confstr('CS_GNU_LIBC_VERSION')` split into `(lib, version)`.
fn host_libc_confstr() -> Option<(String, String)> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use std::ffi::CStr;
        // SAFETY: `gnu_get_libc_version` returns a pointer to a static NUL-terminated
        // version string owned by glibc.
        let ptr = unsafe { libc::gnu_get_libc_version() };
        if ptr.is_null() {
            return None;
        }
        let version = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        Some(("glibc".to_string(), version))
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        None
    }
}

fn host_compiler() -> String {
    format!("Rust ({}/{})", std::env::consts::OS, std::env::consts::ARCH)
}

/// What `file -b` would tell `architecture()` about an executable, read from
/// its ELF / Mach-O / PE header.
struct ExecutableProbe {
    /// `'executable' in fileout or 'shared object' in fileout`.
    supported: bool,
    /// `"32bit"` / `"64bit"`, or empty when the output does not say.
    bits: &'static str,
    /// `"ELF"`, `"WindowsPE"`, `"MSDOS"`, or empty.
    linkage: &'static str,
}

impl ExecutableProbe {
    const UNSUPPORTED: Self = Self {
        supported: false,
        bits: "",
        linkage: "",
    };
}

fn read_u16(bytes: &[u8], at: usize, big: bool) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(if big {
        u16::from_be_bytes(raw)
    } else {
        u16::from_le_bytes(raw)
    })
}

fn read_u32(bytes: &[u8], at: usize, big: bool) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(raw)
    } else {
        u32::from_le_bytes(raw)
    })
}

fn probe_executable(path: &std::path::Path) -> ExecutableProbe {
    let Ok(mut file) = std::fs::File::open(path) else {
        return ExecutableProbe::UNSUPPORTED;
    };
    let mut header = vec![0u8; 4096];
    let mut filled = 0;
    while filled < header.len() {
        match file.read(&mut header[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    header.truncate(filled);
    let bytes = header.as_slice();
    if bytes.len() >= 20 && bytes.starts_with(b"\x7fELF") {
        let bits = match bytes[4] {
            1 => "32bit",
            2 => "64bit",
            _ => "",
        };
        let big = bytes[5] == 2;
        // ET_EXEC / ET_DYN are "executable" / "shared object".
        let supported = matches!(read_u16(bytes, 16, big), Some(2 | 3));
        return ExecutableProbe {
            supported,
            bits: if supported { bits } else { "" },
            linkage: if supported { "ELF" } else { "" },
        };
    }
    if let Some(magic) = read_u32(bytes, 0, true) {
        let thin = match magic {
            0xFEED_FACE => Some((false, true)),
            0xFEED_FACF => Some((true, true)),
            0xCEFA_EDFE => Some((false, false)),
            0xCFFA_EDFE => Some((true, false)),
            _ => None,
        };
        if let Some((is64, big)) = thin {
            // MH_EXECUTE is the only "executable" file type.
            let supported = read_u32(bytes, 12, big) == Some(2);
            return ExecutableProbe {
                supported,
                bits: if supported && is64 { "64bit" } else { "" },
                linkage: "",
            };
        }
        if magic == 0xCAFE_BABE
            && let Some(count) = read_u32(bytes, 4, true)
            && (1..=30).contains(&count)
        {
            let mut any64 = false;
            for index in 0..count as usize {
                if let Some(cputype) = read_u32(bytes, 8 + index * 20, true) {
                    any64 |= cputype & 0x0100_0000 != 0;
                }
            }
            return ExecutableProbe {
                supported: true,
                bits: if any64 { "64bit" } else { "" },
                linkage: "",
            };
        }
    }
    if bytes.starts_with(b"MZ") {
        if let Some(offset) = read_u32(bytes, 0x3c, false)
            && bytes.get(offset as usize..offset as usize + 4) == Some(b"PE\0\0".as_slice())
        {
            return ExecutableProbe {
                supported: true,
                bits: "",
                linkage: "WindowsPE",
            };
        }
        return ExecutableProbe {
            supported: true,
            bits: "",
            linkage: "MSDOS",
        };
    }
    ExecutableProbe::UNSUPPORTED
}

// ── Pieces of CPython's helpers ──────────────────────────────────────────────

/// `str.strip()`.
fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// `_platform(*args)`: the filename-compatible platform string.
fn platform_string(args: &[ExpandedCallArg]) -> Result<String> {
    if let Some(keyword) = args.iter().find_map(|arg| arg.name.as_deref()) {
        return Err(type_error(format!(
            "_platform() got an unexpected keyword argument '{keyword}'"
        )));
    }
    let mut parts = Vec::new();
    for arg in args {
        let Some(text) = arg.value.as_str() else {
            return Err(type_error(format!(
                "object of type '{}' has no len()",
                pyrust_core::error_type_name(&arg.value)
            )));
        };
        if !text.is_empty() {
            parts.push(py_strip(text).to_string());
        }
    }
    let mut platform = parts.join("-");
    for (old, new) in [
        (" ", "_"),
        ("/", "-"),
        ("\\", "-"),
        (":", "-"),
        (";", "-"),
        ("\"", "-"),
        ("(", "-"),
        (")", "-"),
    ] {
        platform = platform.replace(old, new);
    }
    // No need to report 'unknown' information...
    platform = platform.replace("unknown", "");
    // Fold '--'s and remove trailing '-'.
    while platform.contains("--") {
        platform = platform.replace("--", "-");
    }
    loop {
        match platform.chars().next_back() {
            None => return Err(PyError::named("IndexError", "string index out of range")),
            Some('-') => {
                platform.pop();
            }
            Some(_) => break,
        }
    }
    Ok(platform)
}

/// `_norm_version(version, build)`.
fn normalize_version(version: &str, build: &str) -> String {
    let mut parts: Vec<String> = version.split('.').map(str::to_string).collect();
    if !build.is_empty() {
        parts.push(build.to_string());
    }
    let numbers: Option<Vec<String>> = parts
        .iter()
        .map(|part| parse_py_int(part).map(|n| n.to_string()))
        .collect();
    let strings = numbers.unwrap_or(parts);
    strings
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(".")
}

/// The `ver` command output parsed by `_syscmd_ver`'s regular expression
/// `(?:([\w ]+) ([\w.]+) .*\[.* ([\d.]+)\])`.
fn parse_ver_output(info: &str) -> Option<(String, String, String)> {
    let text = info.as_bytes();
    let groups: RefCell<[Span; 3]> = RefCell::new([None; 3]);
    let not_newline = |b: u8| b != b'\n';
    let found = greedy(text, 0, 1, &|b| is_word(b) || b == b' ', &mut |end1| {
        groups.borrow_mut()[0] = Some((0, end1));
        if text.get(end1) != Some(&b' ') {
            return false;
        }
        greedy(
            text,
            end1 + 1,
            1,
            &|b| is_word(b) || b == b'.',
            &mut |end2| {
                groups.borrow_mut()[1] = Some((end1 + 1, end2));
                if text.get(end2) != Some(&b' ') {
                    return false;
                }
                greedy(text, end2 + 1, 0, &not_newline, &mut |p| {
                    if text.get(p) != Some(&b'[') {
                        return false;
                    }
                    greedy(text, p + 1, 0, &not_newline, &mut |q| {
                        if text.get(q) != Some(&b' ') {
                            return false;
                        }
                        greedy(
                            text,
                            q + 1,
                            1,
                            &|b| b.is_ascii_digit() || b == b'.',
                            &mut |end3| {
                                groups.borrow_mut()[2] = Some((q + 1, end3));
                                text.get(end3) == Some(&b']')
                            },
                        )
                    })
                })
            },
        )
    });
    if !found {
        return None;
    }
    let groups = groups.into_inner();
    Some((
        slice_text(text, groups[0])?,
        slice_text(text, groups[1])?,
        slice_text(text, groups[2])?,
    ))
}

/// Unescape the five special characters of the os-release standard.
fn unescape_os_release(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && matches!(next, '\\' | '$' | '"' | '\'' | '`')
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// One os-release line: `^(?P<name>[a-zA-Z0-9_]+)=(?P<quote>["']?)(?P<value>.*)(?P=quote)$`.
fn parse_os_release_line(line: &str) -> Option<(String, String)> {
    let body = line.strip_suffix('\n').unwrap_or(line);
    if body.contains('\n') {
        return None;
    }
    let name_len = body
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    if name_len == 0 || body.as_bytes().get(name_len) != Some(&b'=') {
        return None;
    }
    let name = &body[..name_len];
    let rest = &body[name_len + 1..];
    let value = match rest.chars().next() {
        Some(quote @ ('"' | '\'')) if rest.len() > 1 && rest.ends_with(quote) => {
            &rest[1..rest.len() - 1]
        }
        _ => rest,
    };
    Some((name.to_string(), unescape_os_release(value)))
}

// ── `uname_result` plumbing ──────────────────────────────────────────────────

/// The five stored fields of a `uname_result` instance.
fn uname_raw_fields(this: &Value) -> Result<Vec<Value>> {
    if let Some(backing) = builtin_data_backing(this)
        && let ValueKind::Tuple(items) = backing.kind()
    {
        return Ok(items.to_vec());
    }
    Err(type_error("descriptor requires a 'uname_result' object"))
}

/// `tuple(self)`: the five fields plus the lazily computed `processor`.
fn uname_full_tuple(interp: &mut Interpreter, this: &Value) -> Result<Vec<Value>> {
    let mut items = uname_raw_fields(this)?;
    let processor = interp.get_attr(this, "processor")?;
    items.push(processor);
    Ok(items)
}

// ── Public function bodies ───────────────────────────────────────────────────

fn unknown_as_blank(value: Value) -> Value {
    if streq(&value, "unknown") {
        s("")
    } else {
        value
    }
}

fn do_uname(interp: &mut Interpreter) -> Result<Value> {
    let cache = global(interp, "_uname_cache")?;
    if !cache.is_none() {
        return Ok(cache);
    }
    let sys_platform_name = sys_platform(interp)?;
    // Get some infos from the builtin os.uname API...
    let (mut system, mut node, mut release, mut version, mut machine, any_info) =
        match host_os_uname() {
            Some([system, node, release, version, machine]) => {
                let any = [&system, &node, &release, &version, &machine]
                    .iter()
                    .any(|field| !field.is_empty());
                (
                    s(&system),
                    s(&node),
                    s(&release),
                    s(&version),
                    s(&machine),
                    any,
                )
            }
            None => {
                let node = call_global(interp, "_node", &[])?;
                (s(&sys_platform_name), node, s(""), s(""), s(""), false)
            }
        };

    if !any_info {
        // uname is not available.

        // Try win32_ver() on win32 platforms.
        if streq(&system, "win32") {
            let info = call_global(interp, "win32_ver", &[])?;
            let info = unpack_iterable(interp, &info, 4)?;
            release = info[0].clone();
            version = info[1].clone();
            if !truthy(interp, &machine)? {
                machine = s(&host_machine_win32());
            }
        }

        // Try the 'ver' system command available on some platforms.
        if !(truthy(interp, &release)? && truthy(interp, &version)?) {
            let info = call_global(
                interp,
                "_syscmd_ver",
                &[system.clone(), release.clone(), version.clone()],
            )?;
            let info = unpack_iterable(interp, &info, 3)?;
            system = info[0].clone();
            release = info[1].clone();
            version = info[2].clone();
            // Normalize system to what win32_ver() normally returns
            // (_syscmd_ver() tends to return the vendor name as well).
            if streq(&system, "Microsoft Windows") {
                system = s("Windows");
            } else if streq(&system, "Microsoft") && streq(&release, "Windows") {
                // Under Windows Vista and Windows Server 2008, Microsoft
                // changed the output of the ver command.  The release is no
                // longer printed.  This causes the system and release to be
                // misidentified.
                system = s("Windows");
                if version.as_str().is_some_and(|v| v.starts_with("6.0")) {
                    release = s("Vista");
                } else {
                    release = s("");
                }
            }
        }

        // In case we still don't know anything useful, we'll try to help
        // ourselves.
        if streq(&system, "win32") || streq(&system, "win16") {
            if !truthy(interp, &version)? {
                version = if streq(&system, "win32") {
                    s("32bit")
                } else {
                    s("16bit")
                };
            }
            system = s("Windows");
        } else if system.as_str().is_some_and(|name| name.starts_with("java")) {
            let info = call_global(interp, "java_ver", &[])?;
            let info = unpack_iterable(interp, &info, 4)?;
            let vendor = info[1].clone();
            release = info[0].clone();
            system = s("Java");
            let vminfo = interp.collect_iterable(&info[2])?;
            let joined: Vec<String> = vminfo
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect();
            version = s(&joined.join(", "));
            if !truthy(interp, &version)? {
                version = vendor;
            }
        }
    }

    // System specific extensions.
    if streq(&system, "OpenVMS") {
        // OpenVMS seems to have release and version mixed up.
        if !truthy(interp, &release)? || streq(&release, "0") {
            release = version;
            version = s("");
        }
    }

    // Normalize name.
    if streq(&system, "Microsoft") && streq(&release, "Windows") {
        system = s("Windows");
        release = s("Vista");
    }

    // Replace 'unknown' values with the more portable ''.
    node = unknown_as_blank(node);
    let values = [
        unknown_as_blank(system),
        node,
        unknown_as_blank(release),
        unknown_as_blank(version),
        unknown_as_blank(machine),
    ];
    let class = global(interp, "uname_result")?;
    let result = call(interp, &class, &values)?;
    set_global(interp, "_uname_cache", result.clone())?;
    Ok(result)
}

fn uname_field(interp: &mut Interpreter, field: &str) -> Result<Value> {
    let result = call_global(interp, "uname", &[])?;
    interp.get_attr(&result, field)
}

fn do_architecture(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_ARCHITECTURE, args)?;
    let executable = match bound.take(0) {
        Some(value) => value,
        None => sys_attr(interp, "executable")?,
    };
    let mut bits = bound.take_or(1, || s(""));
    let mut linkage = bound.take_or(2, || s(""));

    // Use the sizeof(pointer) as default number of bits if nothing else is
    // given as default.
    if !truthy(interp, &bits)? {
        bits = s(&format!("{}bit", std::mem::size_of::<usize>() * 8));
    }

    let platform_name = sys_platform(interp)?;
    let default_table = global(interp, "_default_architecture")?;
    let sys_executable = sys_attr(interp, "executable")?;
    let is_sys_executable = py_eq(interp, &executable, &sys_executable)?;

    // `file -b` is never consulted on win32 / win16 / dos, where
    // `_syscmd_file` returns its default and the table below is used.
    let table_hit = if matches!(platform_name.as_str(), "win32" | "win16" | "dos") {
        let key = s(&platform_name);
        Some(method(
            interp,
            &default_table,
            "get",
            &[key, Value::none()],
        )?)
    } else {
        None
    };
    if let Some(entry) = table_hit {
        if is_sys_executable && !entry.is_none() {
            let entry = unpack_iterable(interp, &entry, 2)?;
            if truthy(interp, &entry[0])? {
                bits = entry[0].clone();
            }
            if truthy(interp, &entry[1])? {
                linkage = entry[1].clone();
            }
        }
        return Ok(Value::tuple(vec![bits, linkage]));
    }

    if !truthy(interp, &executable)? {
        return Ok(Value::tuple(vec![bits, linkage]));
    }
    let path = match fspath_resolve(interp, &executable)? {
        PathValue::Str(text) => std::path::PathBuf::from(text),
        PathValue::Bytes(raw) => std::path::PathBuf::from(String::from_utf8_lossy(&raw).as_ref()),
    };
    let probe = probe_executable(&path);
    if !probe.supported {
        // Format not supported.
        return Ok(Value::tuple(vec![bits, linkage]));
    }
    if !probe.bits.is_empty() {
        bits = s(probe.bits);
    }
    if !probe.linkage.is_empty() {
        linkage = s(probe.linkage);
    }
    Ok(Value::tuple(vec![bits, linkage]))
}

fn do_system_alias(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_SYSTEM_ALIAS, args)?;
    let system = bound.take_or_none(0);
    let mut release = bound.take_or_none(1);
    let version = bound.take_or_none(2);
    let mut system = system;
    if streq(&system, "SunOS") {
        // Sun's OS.
        if binary(interp, &release, BinaryOp::Lt, &s("5"))? {
            // These releases use the old name SunOS.
            return Ok(Value::tuple(vec![system, release, version]));
        }
        // Modify release (marketing release = SunOS release - 3).
        let parts = method(interp, &release, "split", &[s(".")])?;
        let mut parts = interp.collect_iterable(&parts)?;
        if let Some(first) = parts.first().and_then(|v| v.as_str())
            && let Some(major) = parse_py_int(first)
        {
            parts[0] = s(&(major - 3).to_string());
            let joined: Vec<String> = parts
                .iter()
                .map(|part| part.as_str().unwrap_or_default().to_string())
                .collect();
            release = s(&joined.join("."));
        }
        // `if release < '6': system = 'Solaris' else: system = 'Solaris'`
        binary(interp, &release, BinaryOp::Lt, &s("6"))?;
        system = s("Solaris");
    } else if streq(&system, "win32") || streq(&system, "win16") {
        // In case one of the other tricks.
        system = s("Windows");
    }
    // bpo-35516: Don't replace Darwin with macOS since input release and
    // version arguments can be different than the currently running version.
    Ok(Value::tuple(vec![system, release, version]))
}

fn do_win32_ver_private(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_WIN32_VER_PRIVATE, args)?;
    let version = bound.take_or_none(0);
    let csd = bound.take_or_none(1);
    let ptype = bound.take_or_none(2);
    let _ = interp;
    #[cfg(windows)]
    {
        if let Some((version, csd, ptype, is_client)) = win32_info::query_win32_ver() {
            return Ok(Value::tuple(vec![
                s(&version),
                s(&csd),
                s(&ptype),
                Value::bool_(is_client),
            ]));
        }
    }
    Ok(Value::tuple(vec![version, csd, ptype, Value::bool_(true)]))
}

fn do_win32_ver(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_WIN32_VER, args)?;
    let mut release = bound.take_or(0, || s(""));
    let version = bound.take_or(1, || s(""));
    let csd = bound.take_or(2, || s(""));
    let ptype = bound.take_or(3, || s(""));

    let info = call_global(interp, "_win32_ver", &[version, csd, ptype])?;
    let info = unpack_iterable(interp, &info, 4)?;
    let version = info[0].clone();
    let csd = info[1].clone();
    let ptype = info[2].clone();
    let is_client = truthy(interp, &info[3])?;

    if truthy(interp, &version)? {
        let parts = method(interp, &version, "split", &[s(".")])?;
        let parts = interp.collect_iterable(&parts)?;
        let mut numbers = Vec::with_capacity(parts.len());
        for part in &parts {
            let text = part.as_str().unwrap_or_default().to_string();
            numbers.push(int_value(py_int(interp, &text)?));
        }
        let int_version = Value::tuple(numbers);
        let table = global(
            interp,
            if is_client {
                "_WIN32_CLIENT_RELEASES"
            } else {
                "_WIN32_SERVER_RELEASES"
            },
        )?;
        for entry in interp.collect_iterable(&table)? {
            let entry = unpack_iterable(interp, &entry, 2)?;
            if binary(interp, &entry[0], BinaryOp::Le, &int_version)? {
                release = entry[1].clone();
                break;
            }
        }
    }
    Ok(Value::tuple(vec![release, version, csd, ptype]))
}

fn do_win32_edition(interp: &mut Interpreter) -> Result<Value> {
    // `import winreg` (absent everywhere but Windows).
    let Ok(winreg) = interp.load_module("winreg") else {
        return Ok(Value::none());
    };
    let attempt = (|| -> Result<Value> {
        let hklm = interp.get_attr(&winreg, "HKEY_LOCAL_MACHINE")?;
        let open = interp.get_attr(&winreg, "OpenKeyEx")?;
        let key = call(
            interp,
            &open,
            &[hklm, s(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")],
        )?;
        let query = interp.get_attr(&winreg, "QueryValueEx")?;
        let result = call(interp, &query, &[key.clone(), s("EditionId")]);
        let closed = method(interp, &key, "Close", &[]);
        let result = result?;
        closed?;
        let first = crate::builtin_modules::operator::getitem_value(interp, result, Value::int(0))?;
        Ok(first)
    })();
    match attempt {
        Ok(value) => Ok(value),
        Err(error) => {
            let (is_os_error, error) = error_is(interp, error, "OSError")?;
            if is_os_error.is_some() {
                Ok(Value::none())
            } else {
                Err(error)
            }
        }
    }
}

fn do_win32_is_iot(interp: &mut Interpreter) -> Result<Value> {
    let edition = call_global(interp, "win32_edition", &[])?;
    Ok(Value::bool_(edition.as_str().is_some_and(|name| {
        matches!(
            name,
            "IoTUAP" | "NanoServer" | "WindowsCoreHeadless" | "IoTEdgeOS"
        )
    })))
}

fn do_mac_ver_xml(interp: &mut Interpreter) -> Result<Value> {
    const PATH: &str = "/System/Library/CoreServices/SystemVersion.plist";
    if !std::path::Path::new(PATH).exists() {
        return Ok(Value::none());
    }
    let text = std::fs::read(PATH).map_err(|error| PyError::from_io_error(&error, Some(PATH)))?;
    let text = String::from_utf8_lossy(&text);
    let release = text
        .split_once("<key>ProductVersion</key>")
        .and_then(|(_, rest)| rest.split_once("<string>"))
        .and_then(|(_, rest)| rest.split_once("</string>"))
        .map(|(value, _)| value.trim().to_string());
    let Some(release) = release else {
        let _ = interp;
        return Err(PyError::key_error(s("ProductVersion")));
    };
    let mut machine = host_os_uname()
        .map(|fields| fields[4].clone())
        .unwrap_or_default();
    if machine == "ppc" || machine == "Power Macintosh" {
        // Canonical name.
        machine = "PowerPC".to_string();
    }
    Ok(Value::tuple(vec![
        s(&release),
        Value::tuple(vec![s(""), s(""), s("")]),
        s(&machine),
    ]))
}

fn do_mac_ver(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_MAC_VER, args)?;
    let release = bound.take_or(0, || s(""));
    let versioninfo = bound.take_or(1, || Value::tuple(vec![s(""), s(""), s("")]));
    let machine = bound.take_or(2, || s(""));
    // First try reading the information from an XML file which should always
    // be present.
    let info = call_global(interp, "_mac_ver_xml", &[])?;
    if !info.is_none() {
        return Ok(info);
    }
    // If that also doesn't work return the default values.
    Ok(Value::tuple(vec![release, versioninfo, machine]))
}

fn do_java_ver(args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_JAVA_VER, args)?;
    let release = bound.take_or(0, || s(""));
    let vendor = bound.take_or(1, || s(""));
    let vminfo = bound.take_or(2, || Value::tuple(vec![s(""), s(""), s("")]));
    let osinfo = bound.take_or(3, || Value::tuple(vec![s(""), s(""), s("")]));
    // `import java.lang` always fails here.
    Ok(Value::tuple(vec![release, vendor, vminfo, osinfo]))
}

// ── libc_ver ─────────────────────────────────────────────────────────────────

/// A match of the `libc_search` regular expression.
struct LibcMatch {
    end: usize,
    libcinit: bool,
    glibc: Option<(usize, usize)>,
    so: bool,
    threads: Option<(usize, usize)>,
    soversion: Option<(usize, usize)>,
}

/// `(__libc_init)|(GLIBC_([0-9.]+))|(libc(_\w+)?\.so(?:\.(\d[0-9.]*))?)`
/// searched from `pos` (`re.ASCII`).
fn libc_search(binary: &[u8], pos: usize) -> Option<LibcMatch> {
    let run = |from: usize, accepts: &dyn Fn(u8) -> bool| -> usize {
        let mut end = from;
        while end < binary.len() && accepts(binary[end]) {
            end += 1;
        }
        end
    };
    for start in pos..binary.len() {
        let rest = &binary[start..];
        if rest.starts_with(b"__libc_init") {
            return Some(LibcMatch {
                end: start + 11,
                libcinit: true,
                glibc: None,
                so: false,
                threads: None,
                soversion: None,
            });
        }
        if rest.starts_with(b"GLIBC_") {
            let from = start + 6;
            let end = run(from, &|b| b.is_ascii_digit() || b == b'.');
            if end > from {
                return Some(LibcMatch {
                    end,
                    libcinit: false,
                    glibc: Some((from, end)),
                    so: false,
                    threads: None,
                    soversion: None,
                });
            }
        }
        if rest.starts_with(b"libc") {
            let after = start + 4;
            let mut threads = None;
            let mut at = after;
            if binary.get(after) == Some(&b'_') {
                let end = run(after + 1, &is_ascii_word);
                if end > after + 1 && binary[end..].starts_with(b".so") {
                    threads = Some((after, end));
                    at = end;
                }
            }
            if binary[at..].starts_with(b".so") {
                let so_end = at + 3;
                let mut end = so_end;
                let mut soversion = None;
                if binary.get(so_end) == Some(&b'.')
                    && binary.get(so_end + 1).is_some_and(u8::is_ascii_digit)
                {
                    end = run(so_end + 2, &|b| b.is_ascii_digit() || b == b'.');
                    soversion = Some((so_end + 1, end));
                }
                return Some(LibcMatch {
                    end,
                    libcinit: false,
                    glibc: None,
                    so: true,
                    threads,
                    soversion,
                });
            }
        }
    }
    None
}

fn read_chunk(file: &mut std::fs::File, size: i64) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    if size < 0 {
        file.read_to_end(&mut buffer)?;
    } else {
        file.take(size as u64).read_to_end(&mut buffer)?;
    }
    Ok(buffer)
}

fn latin1(bytes: &[u8], span: (usize, usize)) -> String {
    bytes[span.0..span.1]
        .iter()
        .map(|&b| char::from(b))
        .collect()
}

fn do_libc_ver(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_LIBC_VER, args)?;
    let mut executable = bound.take_or_none(0);
    let mut lib = bound.take_or(1, || s(""));
    let mut version = bound.take_or(2, || s(""));
    let chunksize = bound.take_or(3, || Value::int(16384));

    if !truthy(interp, &executable)? {
        if let Some((name, number)) = host_libc_confstr() {
            return Ok(Value::tuple(vec![s(&name), s(&number)]));
        }
        executable = sys_attr(interp, "executable")?;
        if !truthy(interp, &executable)? {
            // sys.executable is not set.
            return Ok(Value::tuple(vec![lib, version]));
        }
    }

    // We use os.path.realpath() here to work around problems with Cygwin not
    // being able to open symlinks for reading.
    let path = match fspath_resolve(interp, &executable)? {
        PathValue::Str(text) => std::path::PathBuf::from(text),
        PathValue::Bytes(raw) => std::path::PathBuf::from(String::from_utf8_lossy(&raw).as_ref()),
    };
    let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let display = path.to_string_lossy().into_owned();
    let mut file = std::fs::File::open(&real)
        .map_err(|error| PyError::from_io_error(&error, Some(&display)))?;
    let io = |error: std::io::Error| PyError::from_io_error(&error, None);

    // `f.read(chunksize)`: an int, or None for the whole file.
    let chunksize = match (chunksize.as_int(), chunksize.is_none()) {
        (Some(size), _) => size,
        (None, true) => -1,
        (None, false) => {
            return Err(type_error(format!(
                "argument should be integer or None, not '{}'",
                pyrust_core::error_type_name(&chunksize)
            )));
        }
    };
    let mut binary = read_chunk(&mut file, chunksize).map_err(io)?;
    let mut pos = 0usize;
    while pos < binary.len() {
        let mentions_libc =
            binary.windows(4).any(|w| w == b"libc") || binary.windows(5).any(|w| w == b"GLIBC");
        let found = if mentions_libc {
            libc_search(&binary, pos)
        } else {
            None
        };
        let at_end = found.as_ref().is_none_or(|m| m.end == binary.len());
        if at_end {
            let chunk = read_chunk(&mut file, chunksize).map_err(io)?;
            if !chunk.is_empty() {
                let keep_from = pos.max(binary.len().saturating_sub(1000));
                let mut joined = binary[keep_from..].to_vec();
                joined.extend_from_slice(&chunk);
                binary = joined;
                pos = 0;
                continue;
            }
            if found.is_none() {
                break;
            }
        }
        let Some(found) = found else { break };

        if found.libcinit && !truthy(interp, &lib)? {
            lib = s("libc");
        } else if let Some(span) = found.glibc {
            let glibc_version = latin1(&binary, span);
            if !streq(&lib, "glibc") {
                lib = s("glibc");
                version = s(&glibc_version);
            } else {
                let current = version_text(&version)?;
                if version_greater(&glibc_version, &current) {
                    version = s(&glibc_version);
                }
            }
        } else if found.so && !streq(&lib, "glibc") {
            lib = s("libc");
            if let Some(span) = found.soversion {
                let so_version = latin1(&binary, span);
                let current = version_text(&version)?;
                if current.is_empty() || version_greater(&so_version, &current) {
                    version = s(&so_version);
                }
            }
            if let Some(span) = found.threads {
                let threads = latin1(&binary, span);
                let current = version_text(&version)?;
                if !current.ends_with(&threads) {
                    version = s(&format!("{current}{threads}"));
                }
            }
        }
        pos = found.end;
    }
    Ok(Value::tuple(vec![lib, version]))
}

fn version_text(version: &Value) -> Result<String> {
    version.as_str().map(str::to_string).ok_or_else(|| {
        type_error(format!(
            "can only concatenate str (not \"{}\") to str",
            pyrust_core::error_type_name(version)
        ))
    })
}

// ── sys.version ──────────────────────────────────────────────────────────────

fn do_sys_version(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_SYS_VERSION, args)?;
    let explicit = bound.take(0).filter(|value| !value.is_none());
    let sys_version = match &explicit {
        Some(value) => value.clone(),
        None => sys_attr(interp, "version")?,
    };

    // Try the cache first.
    let cache = global(interp, "_sys_version_cache")?;
    let cached = method(interp, &cache, "get", &[sys_version.clone(), Value::none()])?;
    if !cached.is_none() {
        return Ok(cached);
    }

    let Some(text) = sys_version.as_str().map(str::to_string) else {
        return Err(type_error(format!(
            "argument of type '{}' is not iterable",
            pyrust_core::error_type_name(&sys_version)
        )));
    };
    let platform_name = sys_platform(interp)?;
    let bytes = text.as_bytes();

    let (name, mut version, buildno, builddate, compiler);
    if platform_name.starts_with("java") {
        // Jython.
        let Some(groups) = match_cpython_version(bytes) else {
            let repr = render_value_repr(interp, &sys_version)?;
            return Err(value_error(format!(
                "failed to parse Jython sys.version: {repr}"
            )));
        };
        name = "Jython".to_string();
        version = slice_text(bytes, groups[0]).unwrap_or_default();
        buildno = slice_text(bytes, groups[1]).unwrap_or_default();
        builddate = slice_text(bytes, groups[2]).unwrap_or_default();
        compiler = platform_name.clone();
    } else if text.contains("PyPy") {
        let Some(groups) = match_pypy_version(bytes) else {
            let repr = render_value_repr(interp, &sys_version)?;
            return Err(value_error(format!(
                "failed to parse PyPy sys.version: {repr}"
            )));
        };
        name = "PyPy".to_string();
        version = slice_text(bytes, groups[0]).unwrap_or_default();
        buildno = slice_text(bytes, groups[1]).unwrap_or_default();
        builddate = slice_text(bytes, groups[2]).unwrap_or_default();
        compiler = String::new();
    } else if let Some(groups) = match_cpython_version(bytes) {
        // CPython.
        name = "CPython".to_string();
        version = slice_text(bytes, groups[0]).unwrap_or_default();
        buildno = slice_text(bytes, groups[1]).unwrap_or_default();
        let date = slice_text(bytes, groups[2]);
        let time = slice_text(bytes, groups[3]);
        builddate = match (date, time) {
            (None, _) => String::new(),
            (Some(date), Some(time)) if !time.is_empty() => format!("{date} {time}"),
            (Some(date), _) => date,
        };
        compiler = slice_text(bytes, groups[4]).unwrap_or_default();
    } else if explicit.is_some() {
        let repr = render_value_repr(interp, &sys_version)?;
        return Err(value_error(format!(
            "failed to parse CPython sys.version: {repr}"
        )));
    } else {
        // This runtime's own `sys.version` is not in CPython's shape: derive
        // the answer from `sys.implementation` and `sys.version_info`.
        let implementation = sys_attr(interp, "implementation")?;
        let implementation_name = interp.get_attr(&implementation, "name")?;
        name = match implementation_name.as_str().unwrap_or("cpython") {
            "cpython" => "CPython".to_string(),
            "pypy" => "PyPy".to_string(),
            other => other.to_string(),
        };
        let info = sys_attr(interp, "version_info")?;
        let mut numbers = Vec::new();
        for field in ["major", "minor", "micro"] {
            let value = interp.get_attr(&info, field)?;
            numbers.push(value.as_int().unwrap_or(0).to_string());
        }
        version = numbers.join(".");
        buildno = "main".to_string();
        builddate = String::new();
        compiler = host_compiler();
    }

    let (branch, revision) =
        match sys_attr(interp, "_git").or_else(|_| sys_attr(interp, "_mercurial")) {
            Ok(git) => {
                let parts = unpack_iterable(interp, &git, 3)?;
                (parts[1].clone(), parts[2].clone())
            }
            Err(_) => (s(""), s("")),
        };

    // Add the patchlevel version if missing.
    let mut parts: Vec<&str> = version.split('.').collect();
    if parts.len() == 2 {
        parts.push("0");
        version = parts.join(".");
    }

    // Build and cache the result.
    let result = Value::tuple(vec![
        s(&name),
        s(&version),
        branch,
        revision,
        s(&buildno),
        s(&builddate),
        s(&compiler),
    ]);
    method(
        interp,
        &cache,
        "__setitem__",
        &[sys_version, result.clone()],
    )?;
    Ok(result)
}

/// `_sys_version()[index]`.
fn sys_version_item(interp: &mut Interpreter, index: i64) -> Result<Value> {
    let info = call_global(interp, "_sys_version", &[])?;
    crate::builtin_modules::operator::getitem_value(interp, info, Value::int(index))
}

fn do_python_version_tuple(interp: &mut Interpreter) -> Result<Value> {
    let version = sys_version_item(interp, 1)?;
    let parts = method(interp, &version, "split", &[s(".")])?;
    let items = interp.collect_iterable(&parts)?;
    Ok(Value::tuple(items))
}

fn do_python_build(interp: &mut Interpreter) -> Result<Value> {
    let info = call_global(interp, "_sys_version", &[])?;
    let items = interp.collect_iterable(&info)?;
    Ok(Value::tuple(items.into_iter().skip(4).take(2).collect()))
}

// ── platform() ───────────────────────────────────────────────────────────────

fn do_platform(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_PLATFORM, args)?;
    let aliased = bound.take_or(0, || Value::bool_(false));
    let terse = bound.take_or(1, || Value::bool_(false));

    let cache = global(interp, "_platform_cache")?;
    let key = Value::tuple(vec![aliased.clone(), terse.clone()]);
    let cached = method(interp, &cache, "get", &[key.clone(), Value::none()])?;
    if !cached.is_none() {
        return Ok(cached);
    }

    // Get uname information and then apply platform specific cosmetics to it...
    let info = call_global(interp, "uname", &[])?;
    let info = unpack_iterable(interp, &info, 6)?;
    let mut system = info[0].clone();
    let mut release = info[2].clone();
    let mut version = info[3].clone();
    let machine = info[4].clone();
    let mut processor = info[5].clone();
    if py_eq(interp, &machine, &processor)? {
        processor = s("");
    }
    if truthy(interp, &aliased)? {
        let aliased_info = call_global(
            interp,
            "system_alias",
            &[system.clone(), release.clone(), version.clone()],
        )?;
        let aliased_info = unpack_iterable(interp, &aliased_info, 3)?;
        system = aliased_info[0].clone();
        release = aliased_info[1].clone();
        version = aliased_info[2].clone();
    }

    if streq(&system, "Darwin") {
        // macOS (darwin kernel).
        let mac = call_global(interp, "mac_ver", &[])?;
        let macos_release =
            crate::builtin_modules::operator::getitem_value(interp, mac, Value::int(0))?;
        if truthy(interp, &macos_release)? {
            system = s("macOS");
            release = macos_release;
        }
    }

    let platform_function = global(interp, "_platform")?;
    let platform = if streq(&system, "Windows") {
        // MS platforms.
        let win = call_global(interp, "win32_ver", std::slice::from_ref(&version))?;
        let win = unpack_iterable(interp, &win, 4)?;
        let csd = win[2].clone();
        if truthy(interp, &terse)? {
            call(interp, &platform_function, &[system, release])?
        } else {
            call(interp, &platform_function, &[system, release, version, csd])?
        }
    } else if streq(&system, "Linux") {
        // Check for libc vs. glibc.
        let libc = call_global(interp, "libc_ver", &[])?;
        let libc = unpack_iterable(interp, &libc, 2)?;
        let with = interp.eval_binary(libc[0].clone(), BinaryOp::Add, libc[1].clone())?;
        call(
            interp,
            &platform_function,
            &[system, release, machine, processor, s("with"), with],
        )?
    } else if streq(&system, "Java") {
        // Java platforms.
        let java = call_global(interp, "java_ver", &[])?;
        let java = unpack_iterable(interp, &java, 4)?;
        let os_info = unpack_iterable(interp, &java[3], 3)?;
        if truthy(interp, &terse)? || !truthy(interp, &os_info[0])? {
            call(interp, &platform_function, &[system, release, version])?
        } else {
            call(
                interp,
                &platform_function,
                &[
                    system,
                    release,
                    version,
                    s("on"),
                    os_info[0].clone(),
                    os_info[1].clone(),
                    os_info[2].clone(),
                ],
            )?
        }
    } else if truthy(interp, &terse)? {
        // Generic handler.
        call(interp, &platform_function, &[system, release])?
    } else {
        let executable = sys_attr(interp, "executable")?;
        let architecture = call_global(interp, "architecture", &[executable])?;
        let architecture = unpack_iterable(interp, &architecture, 2)?;
        call(
            interp,
            &platform_function,
            &[
                system,
                release,
                machine,
                processor,
                architecture[0].clone(),
                architecture[1].clone(),
            ],
        )?
    };

    method(interp, &cache, "__setitem__", &[key, platform.clone()])?;
    Ok(platform)
}

// ── freedesktop os-release ───────────────────────────────────────────────────

fn do_parse_os_release(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_PARSE_OS_RELEASE, args)?;
    let lines = bound.take_or_none(0);
    // These fields are mandatory fields with well-known defaults in practice
    // all Linux distributions override NAME, ID, and PRETTY_NAME.
    let info = new_dict();
    dict_set(&info, "NAME", s("Linux"))?;
    dict_set(&info, "ID", s("linux"))?;
    dict_set(&info, "PRETTY_NAME", s("Linux"))?;
    for line in interp.collect_iterable(&lines)? {
        let Some(text) = line.as_str() else {
            return Err(type_error(format!(
                "expected string or bytes-like object, got '{}'",
                pyrust_core::error_type_name(&line)
            )));
        };
        if let Some((name, value)) = parse_os_release_line(text) {
            dict_set(&info, &name, s(&value))?;
        }
    }
    Ok(info)
}

fn do_freedesktop_os_release(interp: &mut Interpreter) -> Result<Value> {
    let mut cache = global(interp, "_os_release_cache")?;
    if cache.is_none() {
        let candidates = global(interp, "_os_release_candidates")?;
        let candidates = interp.collect_iterable(&candidates)?;
        let builtins = interp.load_module("builtins")?;
        let open = interp.get_attr(&builtins, "open")?;
        let mut errno = Value::none();
        let mut parsed = None;
        for candidate in &candidates {
            let attempt = (|| -> Result<Value> {
                let file = call_kw(
                    interp,
                    &open,
                    std::slice::from_ref(candidate),
                    &[("encoding", s("utf-8"))],
                )?;
                let parsed = call_global(interp, "_parse_os_release", std::slice::from_ref(&file));
                let closed = method(interp, &file, "close", &[]);
                let parsed = parsed?;
                closed?;
                Ok(parsed)
            })();
            match attempt {
                Ok(value) => {
                    parsed = Some(value);
                    break;
                }
                Err(error) => {
                    let (os_error, error) = error_is(interp, error, "OSError")?;
                    let Some(exception) = os_error else {
                        return Err(error);
                    };
                    errno = interp.get_attr(&exception, "errno")?;
                }
            }
        }
        let Some(parsed) = parsed else {
            let mut names = Vec::new();
            for candidate in &candidates {
                names.push(candidate.as_str().unwrap_or_default().to_string());
            }
            let class = lookup_exc_class("OSError")
                .ok_or_else(|| PyError::runtime("platform: OSError is missing".to_string()))?;
            let exception = call(
                interp,
                &Value::py_class(class),
                &[
                    errno,
                    s(&format!("Unable to read files {}", names.join(", "))),
                ],
            )?;
            return Err(PyError::raised(exception));
        };
        set_global(interp, "_os_release_cache", parsed.clone())?;
        cache = parsed;
    }
    method(interp, &cache, "copy", &[])
}

// ── Post-load wiring ─────────────────────────────────────────────────────────

fn ver_stages() -> Value {
    let stages = new_dict();
    for (name, value) in [
        ("dev", 10),
        ("alpha", 20),
        ("a", 20),
        ("beta", 30),
        ("b", 30),
        ("c", 40),
        ("RC", 50),
        ("rc", 50),
        ("pl", 200),
        ("p", 200),
    ] {
        let _ = dict_set(&stages, name, Value::int(value));
    }
    stages
}

fn release_table(entries: &[((i64, i64, i64), &str)]) -> Value {
    Value::list(
        entries
            .iter()
            .map(|&((major, minor, build), name)| {
                Value::tuple(vec![
                    Value::tuple(vec![
                        Value::int(major),
                        Value::int(minor),
                        Value::int(build),
                    ]),
                    s(name),
                ])
            })
            .collect(),
    )
}

fn win32_client_releases() -> Value {
    release_table(&[
        ((10, 1, 0), "post11"),
        ((10, 0, 22000), "11"),
        ((6, 4, 0), "10"),
        ((6, 3, 0), "8.1"),
        ((6, 2, 0), "8"),
        ((6, 1, 0), "7"),
        ((6, 0, 0), "Vista"),
        ((5, 2, 3790), "XP64"),
        ((5, 2, 0), "XPMedia"),
        ((5, 1, 0), "XP"),
        ((5, 0, 0), "2000"),
    ])
}

fn win32_server_releases() -> Value {
    release_table(&[
        ((10, 1, 0), "post2022Server"),
        ((10, 0, 20348), "2022Server"),
        ((10, 0, 17763), "2019Server"),
        ((6, 4, 0), "2016Server"),
        ((6, 3, 0), "2012ServerR2"),
        ((6, 2, 0), "2012Server"),
        ((6, 1, 0), "2008ServerR2"),
        ((6, 0, 0), "2008Server"),
        ((5, 2, 0), "2003Server"),
        ((5, 0, 0), "2000Server"),
    ])
}

fn default_architecture() -> Value {
    let table = new_dict();
    for (platform, linkage) in [
        ("win32", "WindowsPE"),
        ("win16", "Windows"),
        ("dos", "MSDOS"),
    ] {
        let _ = dict_set(&table, platform, Value::tuple(vec![s(""), s(linkage)]));
    }
    table
}

fn os_release_candidates() -> Value {
    Value::tuple(vec![s("/etc/os-release"), s("/usr/lib/os-release")])
}

/// Build the `uname_result` base class, wire the native class onto it and
/// restore the namespace order of `platform.py`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let collections = interp.load_module("collections")?;
    let functools = interp.load_module("functools")?;
    let os = interp.load_module("os")?;
    let sys = interp.load_module("sys")?;
    let itertools = interp.load_module("itertools")?;
    let namedtuple = interp.get_attr(&collections, "namedtuple")?;

    // class uname_result(collections.namedtuple("uname_result_base",
    //                    "system node release version machine")):
    let base = call(
        interp,
        &namedtuple,
        &[
            s("uname_result_base"),
            s("system node release version machine"),
        ],
    )?;
    interp.assign_attr(base.clone(), "__module__", s("platform"))?;
    let ValueKind::PyClass(base_class) = base.kind() else {
        return Err(PyError::runtime(
            "platform: namedtuple() did not return a class".to_string(),
        ));
    };

    let class_value = module.borrow().get_attr_value("uname_result");
    let processor_function = module.borrow().get_attr_value("_uname_result_processor");
    let (Some(class_value), Some(processor_function)) = (class_value, processor_function) else {
        return Err(PyError::runtime(
            "platform: uname_result is missing".to_string(),
        ));
    };
    let ValueKind::PyClass(class) = class_value.kind() else {
        return Err(PyError::runtime(
            "platform: uname_result is not a class".to_string(),
        ));
    };
    link_class_bases(class, vec![Rc::clone(base_class)]);

    // @functools.cached_property def processor(self): ...
    let cached_property = interp.get_attr(&functools, "cached_property")?;
    let processor = call(interp, &cached_property, &[processor_function])?;
    method(
        interp,
        &processor,
        "__set_name__",
        &[class_value.clone(), s("processor")],
    )?;
    {
        let mut class = class.borrow_mut();
        class.attrs.insert("processor".to_string(), processor);
        class
            .attrs
            .insert("__doc__".to_string(), s(UNAME_RESULT_DOC));
        class.bump_mutation_version();
    }

    let mut module = module.borrow_mut();
    module.attrs.shift_remove("_uname_result_processor");
    for (name, value) in [
        ("collections", collections),
        ("os", os),
        ("sys", sys),
        ("functools", functools),
        ("itertools", itertools),
    ] {
        module.attrs.insert(name.to_string(), value);
    }
    for name in MODULE_ORDER {
        if let Some(value) = module.attrs.shift_remove(*name) {
            module.attrs.insert((*name).to_string(), value);
        }
    }
    Ok(())
}

fn uname_fields_tuple() -> Value {
    Value::tuple(UNAME_FIELDS.iter().map(|field| s(field)).collect())
}

pyrust_module! {
    constants {
        "__doc__" => s(MODULE_DOC),
        "__copyright__" => s(COPYRIGHT),
        "__version__" => s("1.0.8"),
        "_ver_stages" => ver_stages(),
        "_WIN32_CLIENT_RELEASES" => win32_client_releases(),
        "_WIN32_SERVER_RELEASES" => win32_server_releases(),
        "_default_architecture" => default_architecture(),
        "_uname_cache" => Value::none(),
        "_sys_version_cache" => new_dict(),
        "_platform_cache" => new_dict(),
        "_os_release_candidates" => os_release_candidates(),
        "_os_release_cache" => Value::none(),
    }

    fn _comparable_version(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_COMPARABLE_VERSION, args)?;
        let version = bound.take_or_none(0);
        let Some(text) = version.as_str() else {
            return Err(type_error(format!(
                "expected string or bytes-like object, got '{}'",
                pyrust_core::error_type_name(&version)
            )));
        };
        let mut items = Vec::new();
        for (stage, part) in comparable_version(text) {
            items.push(Value::int(stage));
            items.push(match part {
                VersionPart::Number(digits) => digits
                    .parse::<i64>()
                    .map_or_else(|_| Value::string(&digits), Value::int),
                VersionPart::Text(text) => Value::string(text),
            });
        }
        Ok(Value::list(items))
    }

    fn libc_ver(args) -> Result<Value> {
        do_libc_ver(_interp, args)
    }

    fn _norm_version(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_NORM_VERSION, args)?;
        let version = bound.take_or_none(0);
        let build = bound.take_or(1, || Value::string(""));
        let Some(text) = version.as_str() else {
            return Err(PyError::named(
                "AttributeError",
                format!(
                    "'{}' object has no attribute 'split'",
                    pyrust_core::error_type_name(&version)
                ),
            ));
        };
        let build = build.as_str().unwrap_or_default();
        Ok(Value::string(normalize_version(text, build)))
    }

    fn _syscmd_ver(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_SYSCMD_VER, args)?;
        let system = bound.take_or(0, || Value::string(""));
        let release = bound.take_or(1, || Value::string(""));
        let version = bound.take_or(2, || Value::string(""));
        let supported = bound.take_or(3, || {
            Value::tuple(vec![s("win32"), s("win16"), s("dos")])
        });
        let platform_name = sys_platform(_interp)?;
        let mut is_supported = false;
        for candidate in _interp.collect_iterable(&supported)? {
            if streq(&candidate, &platform_name) {
                is_supported = true;
                break;
            }
        }
        let given = Value::tuple(vec![system, release, version]);
        if !is_supported {
            return Ok(given);
        }
        // Try some common cmd strings.
        let output = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let Ok(output) = output else {
            return Ok(given);
        };
        if !output.status.success() {
            return Ok(given);
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let Some((system, mut release, mut version)) = parse_ver_output(py_strip(&text)) else {
            return Ok(given);
        };
        // Strip trailing dots from version and release.
        if release.ends_with('.') {
            release.pop();
        }
        if version.ends_with('.') {
            version.pop();
        }
        // Normalize the version and build strings (eliminating additional zeros).
        let version = normalize_version(&version, "");
        Ok(Value::tuple(vec![s(&system), s(&release), s(&version)]))
    }

    fn win32_is_iot(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_win32_is_iot(_interp)
    }

    fn win32_edition(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_win32_edition(_interp)
    }

    fn _win32_ver(args) -> Result<Value> {
        do_win32_ver_private(_interp, args)
    }

    fn win32_ver(args) -> Result<Value> {
        do_win32_ver(_interp, args)
    }

    fn _mac_ver_xml(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_mac_ver_xml(_interp)
    }

    fn mac_ver(args) -> Result<Value> {
        do_mac_ver(_interp, args)
    }

    fn java_ver(args) -> Result<Value> {
        do_java_ver(args)
    }

    fn system_alias(args) -> Result<Value> {
        do_system_alias(_interp, args)
    }

    fn _platform(args) -> Result<Value> {
        Ok(Value::string(platform_string(args)?))
    }

    fn _node(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_NODE, args)?;
        let default = bound.take_or(0, || Value::string(""));
        Ok(host_node().map_or(default, Value::string))
    }

    fn architecture(args) -> Result<Value> {
        do_architecture(_interp, args)
    }

    fn _unknown_as_blank(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_UNKNOWN_AS_BLANK, args)?;
        Ok(unknown_as_blank(bound.take_or_none(0)))
    }

    fn uname(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_uname(_interp)
    }

    fn system(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        uname_field(_interp, "system")
    }

    fn node(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        uname_field(_interp, "node")
    }

    fn release(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        uname_field(_interp, "release")
    }

    fn version(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        uname_field(_interp, "version")
    }

    fn machine(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        uname_field(_interp, "machine")
    }

    fn processor(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        uname_field(_interp, "processor")
    }

    fn _sys_version(args) -> Result<Value> {
        do_sys_version(_interp, args)
    }

    fn python_implementation(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        sys_version_item(_interp, 0)
    }

    fn python_version(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        sys_version_item(_interp, 1)
    }

    fn python_version_tuple(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_python_version_tuple(_interp)
    }

    fn python_branch(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        sys_version_item(_interp, 2)
    }

    fn python_revision(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        sys_version_item(_interp, 3)
    }

    fn python_build(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_python_build(_interp)
    }

    fn python_compiler(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        sys_version_item(_interp, 6)
    }

    fn platform(args) -> Result<Value> {
        do_platform(_interp, args)
    }

    fn _parse_os_release(args) -> Result<Value> {
        do_parse_os_release(_interp, args)
    }

    fn freedesktop_os_release(args) -> Result<Value> {
        no_args(FN_NAME, args)?;
        do_freedesktop_os_release(_interp)
    }

    // The body of `uname_result.processor` (a `functools.cached_property`).
    fn _uname_result_processor(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_SELF_ONLY, args)?;
        let _this = bound.take_or_none(0);
        let platform_name = sys_platform(_interp)?;
        let processor = host_processor(&platform_name);
        Ok(unknown_as_blank(Value::string(processor)))
    }

    class uname_result {
        constants {
            "_fields" => uname_fields_tuple(),
        }

        fn __iter__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELF_ONLY, args)?;
            let this = bound.take_or_none(0);
            let items = uname_full_tuple(_interp, &this)?;
            make_iterator(_interp, &Value::tuple(items))
        }

        fn __getitem__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GETITEM, args)?;
            let this = bound.take_or_none(0);
            let key = bound.take_or_none(1);
            let items = uname_full_tuple(_interp, &this)?;
            crate::builtin_modules::operator::getitem_value(_interp, Value::tuple(items), key)
        }

        fn __len__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELF_ONLY, args)?;
            let this = bound.take_or_none(0);
            let items = uname_full_tuple(_interp, &this)?;
            Ok(Value::int(items.len() as i64))
        }

        fn __repr__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELF_ONLY, args)?;
            let this = bound.take_or_none(0);
            let fields = uname_raw_fields(&this)?;
            let class = _interp.get_attr(&this, "__class__")?;
            let class_name = _interp.get_attr(&class, "__name__")?;
            let mut parts = Vec::with_capacity(fields.len());
            for (name, value) in UNAME_FIELDS.iter().zip(fields.iter()) {
                parts.push(format!("{name}={}", render_value_repr(_interp, value)?));
            }
            Ok(Value::string(format!(
                "{}({})",
                class_name.as_str().unwrap_or("uname_result"),
                parts.join(", ")
            )))
        }

        fn __reduce__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELF_ONLY, args)?;
            let this = bound.take_or_none(0);
            let fields = uname_raw_fields(&this)?;
            let class = global(_interp, "uname_result")?;
            Ok(Value::tuple(vec![class, Value::tuple(fields)]))
        }

        // Override factory to affect length check.
        #[classmethod]
        fn _make(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_MAKE, args)?;
            let cls = bound.take_or_none(0);
            let iterable = bound.take_or_none(1);
            let mut values = vec![cls.clone()];
            values.extend(_interp.collect_iterable(&iterable)?);
            let new = _interp.get_attr(&cls, "__new__")?;
            call(_interp, &new, &values)
        }
    }
}
