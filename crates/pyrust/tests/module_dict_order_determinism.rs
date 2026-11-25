//! Issue #2918: a built-in module's `__dict__` must iterate in the same order
//! in every process.
//!
//! `PyModule::attrs` used to be a `HashMap`, whose iteration order depends on
//! the per-process `RandomState` seed, so `list(vars(math))` differed run to
//! run. Insertion-ordered storage makes it the module's declaration order.
//!
//! This has to spawn the real binary repeatedly: the defect was *cross-process*
//! nondeterminism, which is invisible to any in-process assertion (a `HashMap`
//! iterates consistently within one process) and to the parity harness, which
//! runs each fixture once. The companion parity fixture
//! `tests/cases/stdlib/test_module_dict_order.py` pins the CPython-matching
//! shape of that order.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Every module declared by `pyrust_builtin_modules!`, so that reintroducing
/// hash-ordered storage anywhere is caught. Keep this list in step with that
/// macro — a module missing here is a module whose namespace order is unpinned.
///
/// The list deliberately spans all the distinct namespace kinds: `math` keeps
/// the compact direct `attrs` storage and synthesises `__dict__`; `sys` moves
/// its namespace into a live dict (`attach_live_namespace`); `builtins` is
/// composed from several sub-module attr maps *plus* the exception-class table,
/// which was a second, separate hash-ordered source. The dotted names
/// (`os.path`, `collections.abc`) are bound through their parent package rather
/// than `__import__`, which returns the top-level module for a dotted name.
const PROBE: &str = r#"
import math
import os.path
import collections.abc
import email.header
import email.message
import email.parser
import email.policy
import email.utils
import encodings.aliases
import encodings.ascii
import encodings.base64_codec
import encodings.cp1252
import encodings.hex_codec
import encodings.idna
import encodings.latin_1
import encodings.punycode
import encodings.raw_unicode_escape
import encodings.rot_13
import encodings.unicode_escape
import encodings.utf_7
import encodings.utf_8
import encodings.utf_8_sig
import encodings.utf_16
import encodings.utf_16_be
import encodings.utf_16_le
import encodings.utf_32
import encodings.utf_32_be
import encodings.utf_32_le
import encodings.zlib_codec
import html.entities
import html.parser
import http.client
import http.cookies
import http.server
import importlib.abc
import importlib.machinery
import importlib.metadata
import importlib.readers
import importlib.resources
import importlib.resources.abc
import importlib.resources.readers
import importlib.resources.simple
import importlib.simple
import importlib.util
import sqlite3.dbapi2
import unittest.mock
import urllib.error
import urllib.parse
import urllib.request
import urllib.response
import xml.etree
import xml.etree.ElementPath
import xml.etree.ElementTree
import __future__
names = ["_abc", "_ast", "_asyncio", "_bisect", "_blake2", "_bz2", "_codecs", "_collections", "_collections_abc",
         "_compat_pickle", "_compression", "_contextvars", "_csv", "_datetime", "_decimal", "_elementtree",
         "_frozen_importlib", "_frozen_importlib_external", "_functools", "_hashlib", "_heapq", "_imp", "_io", "_json", "_locale", "_lsprof", "_lzma", "_markupbase", "_md5",
         "_opcode", "_operator", "_pickle", "_queue", "_random", "_sha1", "_sha2", "_sha3", "_signal", "_sitebuiltins",
         "_socket", "_sqlite3", "_sre", "_stat", "_statistics", "_string", "_strptime", "_struct", "_thread",
         "_threading_local", "_tokenize", "_tracemalloc", "_typing", "_uuid", "_warnings", "_weakref", "_weakrefset", "_zoneinfo",
         "abc", "antigravity", "argparse", "ast", "asyncio", "atexit", "base64", "bdb", "binascii", "bisect", "builtins",
         "bz2", "cProfile", "calendar", "cmath", "cmd", "code", "codecs", "codeop", "collections", "colorsys", "configparser", "contextlib", "contextvars",
         "copy", "copyreg", "csv", "dataclasses", "decimal", "difflib", "dis", "doctest", "email", "encodings", "enum", "errno", "faulthandler",
         "filecmp", "fileinput", "fnmatch", "fractions", "functools", "gc", "genericpath", "getopt", "getpass", "gettext", "glob", "graphlib", "gzip", "hashlib", "heapq",
         "hmac", "html", "http", "importlib", "inspect", "io", "ipaddress", "itertools", "json", "keyword",
         "linecache", "locale", "logging", "lzma", "marshal", "math", "mimetypes", "mmap", "netrc", "ntpath", "nturl2path", "numbers", "opcode", "operator", "optparse", "os", "pathlib", "pdb", "pickle",
         "pickletools", "pkgutil", "platform", "posixpath", "pprint", "profile", "pstats", "queue", "quopri", "random", "re", "reprlib", "rlcompleter", "runpy", "sched", "secrets", "select", "selectors", "shlex", "shutil",
         "signal", "site", "socket", "socketserver", "sqlite3", "stat", "statistics", "string", "stringprep", "subprocess", "sys", "sysconfig", "tarfile",
         "tempfile", "textwrap", "this", "threading", "time", "timeit", "token", "tokenize", "tomllib", "trace", "traceback", "tracemalloc",
         "types", "typing", "unicodedata", "unittest", "urllib", "uuid", "warnings", "weakref", "webbrowser",
         "xml", "zipfile", "zlib", "zoneinfo"]
for name in names:
    print(name, list(vars(__import__(name))))
for name, module in [("os.path", os.path),
                     ("collections.abc", collections.abc),
                     ("email.header", email.header),
                     ("email.message", email.message),
                     ("email.parser", email.parser),
                     ("email.policy", email.policy),
                     ("email.utils", email.utils),
                     ("encodings.aliases", encodings.aliases),
                     ("encodings.ascii", encodings.ascii),
                     ("encodings.base64_codec", encodings.base64_codec),
                     ("encodings.cp1252", encodings.cp1252),
                     ("encodings.hex_codec", encodings.hex_codec),
                     ("encodings.idna", encodings.idna),
                     ("encodings.latin_1", encodings.latin_1),
                     ("encodings.punycode", encodings.punycode),
                     ("encodings.raw_unicode_escape", encodings.raw_unicode_escape),
                     ("encodings.rot_13", encodings.rot_13),
                     ("encodings.unicode_escape", encodings.unicode_escape),
                     ("encodings.utf_7", encodings.utf_7),
                     ("encodings.utf_8", encodings.utf_8),
                     ("encodings.utf_8_sig", encodings.utf_8_sig),
                     ("encodings.utf_16", encodings.utf_16),
                     ("encodings.utf_16_be", encodings.utf_16_be),
                     ("encodings.utf_16_le", encodings.utf_16_le),
                     ("encodings.utf_32", encodings.utf_32),
                     ("encodings.utf_32_be", encodings.utf_32_be),
                     ("encodings.utf_32_le", encodings.utf_32_le),
                     ("encodings.zlib_codec", encodings.zlib_codec),
                     ("html.entities", html.entities),
                     ("html.parser", html.parser),
                     ("http.client", http.client),
                     ("http.cookies", http.cookies),
                     ("http.server", http.server),
                     ("importlib.abc", importlib.abc),
                     ("importlib.machinery", importlib.machinery),
                     ("importlib.metadata", importlib.metadata),
                     ("importlib.readers", importlib.readers),
                     ("importlib.resources", importlib.resources),
                     ("importlib.resources.abc", importlib.resources.abc),
                     ("importlib.resources.readers", importlib.resources.readers),
                     ("importlib.resources.simple", importlib.resources.simple),
                     ("importlib.simple", importlib.simple),
                     ("importlib.util", importlib.util),
                     ("sqlite3.dbapi2", sqlite3.dbapi2),
                     ("unittest.mock", unittest.mock),
                     ("urllib.error", urllib.error),
                     ("urllib.parse", urllib.parse),
                     ("urllib.request", urllib.request),
                     ("urllib.response", urllib.response),
                     ("xml.etree", xml.etree),
                     ("xml.etree.ElementPath", xml.etree.ElementPath),
                     ("xml.etree.ElementTree", xml.etree.ElementTree),
                     ("__future__", __future__)]:
    print(name, list(vars(module)))
print(list(math.__dict__))
"#;

const STAR_IMPORT_PROBE: &str = r#"
from math import *
print([name for name in globals() if not name.startswith('__')])
"#;

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

fn write_probe(name: &str, source: &str) -> PathBuf {
    let mut path = env::temp_dir();
    path.push(format!("pyrust_{}_{}.py", name, std::process::id()));
    std::fs::write(&path, source).expect("write probe script");
    path
}

fn run_probe(path: &PathBuf) -> String {
    let output = Command::new(pyrust_bin())
        .arg(path)
        .output()
        .expect("run pyrust");
    assert!(
        output.status.success(),
        "probe script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("probe output is utf-8")
}

/// Five separate processes, each with its own hash seed, must agree exactly.
fn assert_stable_across_runs(name: &str, source: &str) {
    let path = write_probe(name, source);
    let first = run_probe(&path);
    assert!(
        first.lines().count() >= 1 && !first.trim().is_empty(),
        "{name}: probe produced no listing"
    );
    for run in 2..=5 {
        let again = run_probe(&path);
        assert_eq!(
            first, again,
            "{name}: module namespace iteration order changed between run 1 and run {run}; \
             the backing storage must be insertion ordered, not hash ordered"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn builtin_module_dict_order_is_identical_across_processes() {
    assert_stable_across_runs("module_dict_order", PROBE);
}

#[test]
fn star_import_binding_order_is_identical_across_processes() {
    assert_stable_across_runs("star_import_order", STAR_IMPORT_PROBE);
}
