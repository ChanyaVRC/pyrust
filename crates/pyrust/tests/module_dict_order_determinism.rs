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
/// composed from its function families *plus* deferred class slots (the types
/// and the exception-name table, issue #3294) that are only merged into the
/// namespace when it is observed. The dotted names
/// (`os.path`, `collections.abc`) are bound through their parent package rather
/// than `__import__`, which returns the top-level module for a dotted name.
const PROBE: &str = r#"
import math
import os.path
import asyncio.base_events
import asyncio.base_futures
import asyncio.base_subprocess
import asyncio.base_tasks
import asyncio.constants
import asyncio.coroutines
import asyncio.events
import asyncio.exceptions
import asyncio.format_helpers
import asyncio.futures
import asyncio.locks
import asyncio.log
import asyncio.mixins
import asyncio.protocols
import asyncio.queues
import asyncio.runners
import asyncio.selector_events
import asyncio.staggered
import asyncio.streams
import asyncio.subprocess
import asyncio.taskgroups
import asyncio.tasks
import asyncio.threads
import asyncio.timeouts
import asyncio.transports
import asyncio.trsock
import collections.abc
import concurrent.futures
import concurrent.futures._base
import concurrent.futures.process
import concurrent.futures.thread
import ctypes._endian
import ctypes.util
import ctypes.wintypes
import dbm.dumb
import email.base64mime
import email.charset
import email.contentmanager
import email.encoders
import email.errors
import email.feedparser
import email.generator
import email.header
import email.headerregistry
import email.iterators
import email.message
import email.mime
import email.mime.application
import email.mime.audio
import email.mime.base
import email.mime.image
import email.mime.message
import email.mime.multipart
import email.mime.nonmultipart
import email.mime.text
import email.parser
import email.policy
import email.quoprimime
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
import ensurepip.__main__
import ensurepip._uninstall
import html.entities
import html.parser
import http.client
import http.cookiejar
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
import pydoc_data.topics
import json.decoder
import json.encoder
import json.scanner
import json.tool
import logging.config
import logging.handlers
import multiprocessing.connection
import multiprocessing.context
import multiprocessing.dummy
import multiprocessing.dummy.connection
import multiprocessing.heap
import multiprocessing.managers
import multiprocessing.pool
import multiprocessing.process
import multiprocessing.queues
import multiprocessing.reduction
import multiprocessing.resource_sharer
import multiprocessing.resource_tracker
import multiprocessing.shared_memory
import multiprocessing.sharedctypes
import multiprocessing.spawn
import multiprocessing.synchronize
import multiprocessing.util
import sqlite3.dbapi2
import sqlite3.dump
import unittest.mock
import urllib.error
import urllib.parse
import urllib.request
import urllib.response
import urllib.robotparser
import wsgiref.handlers
import wsgiref.headers
import wsgiref.simple_server
import wsgiref.types
import wsgiref.util
import wsgiref.validate
import pyexpat.errors
import pyexpat.model
import xml.dom
import xml.dom.NodeFilter
import xml.dom.domreg
import xml.dom.expatbuilder
import xml.dom.minicompat
import xml.dom.minidom
import xml.dom.pulldom
import xml.dom.xmlbuilder
import xml.etree
import xml.etree.ElementInclude
import xml.etree.ElementPath
import xml.etree.ElementTree
import xml.etree.cElementTree
import xml.parsers
import xml.parsers.expat
import xml.sax
import xml.sax.expatreader
import xml.sax.handler
import xml.sax.saxutils
import xml.sax.xmlreader
import xmlrpc.client
import xmlrpc.server
import __future__
names = ["_abc", "_ast", "_asyncio", "_bisect", "_blake2", "_bz2", "_codecs", "_collections", "_collections_abc",
         "_compat_pickle", "_compression", "_contextvars", "_csv", "_ctypes", "_datetime", "_decimal", "_elementtree",
         "_frozen_importlib", "_frozen_importlib_external", "_functools", "_hashlib", "_heapq", "_imp", "_io", "_json", "_locale", "_lsprof", "_lzma", "_markupbase", "_md5", "_multiprocessing",
         "_opcode", "_operator", "_pickle", "_queue", "_random", "_sha1", "_sha2", "_sha3", "_signal", "_sitebuiltins",
         "_socket", "_sqlite3", "_sre", "_ssl", "_stat", "_statistics", "_string", "_strptime", "_struct", "_symtable", "_thread",
         "_threading_local", "_tokenize", "_tracemalloc", "_typing", "_uuid", "_warnings", "_weakref", "_weakrefset", "_zoneinfo",
         "abc", "antigravity", "argparse", "array", "ast", "asyncio", "atexit", "base64", "bdb", "binascii", "bisect", "builtins",
         "bz2", "cProfile", "calendar", "cmath", "cmd", "code", "codecs", "codeop", "collections", "colorsys", "compileall", "concurrent", "configparser", "contextlib", "contextvars",
         "copy", "copyreg", "csv", "ctypes", "dataclasses", "dbm", "decimal", "difflib", "dis", "doctest", "email", "encodings", "ensurepip", "enum", "errno", "faulthandler",
         "filecmp", "fileinput", "fnmatch", "fractions", "ftplib", "functools", "gc", "genericpath", "getopt", "getpass", "gettext", "glob", "graphlib", "gzip", "hashlib", "heapq",
         "hmac", "html", "http", "imaplib", "importlib", "inspect", "io", "ipaddress", "itertools", "json", "keyword",
         "linecache", "locale", "logging", "lzma", "mailbox", "marshal", "math", "mimetypes", "mmap", "modulefinder", "multiprocessing", "netrc", "ntpath", "nturl2path", "numbers", "opcode", "operator", "optparse", "os", "pathlib", "pdb", "pickle",
         "pickletools", "pkgutil", "platform", "plistlib", "poplib", "posixpath", "pprint", "profile", "pstats", "py_compile", "pyclbr", "pydoc", "pydoc_data", "pyexpat", "queue", "quopri", "random", "re", "reprlib", "rlcompleter", "runpy", "sched", "secrets", "select", "selectors", "shelve", "shlex", "shutil",
         "signal", "site", "smtplib", "socket", "socketserver", "sqlite3", "ssl", "stat", "statistics", "string", "stringprep", "subprocess", "symtable", "sys", "sysconfig", "tabnanny", "tarfile",
         "tempfile", "textwrap", "this", "threading", "time", "timeit", "token", "tokenize", "tomllib", "trace", "traceback", "tracemalloc",
         "types", "typing", "unicodedata", "unittest", "urllib", "uuid", "venv", "warnings", "wave", "weakref", "webbrowser", "wsgiref",
         "xml", "xmlrpc", "zipapp", "zipfile", "zipimport", "zlib", "zoneinfo"]
for name in names:
    print(name, list(vars(__import__(name))))
for name, module in [("os.path", os.path),
                     ("asyncio.base_events", asyncio.base_events),
                     ("asyncio.base_futures", asyncio.base_futures),
                     ("asyncio.base_subprocess", asyncio.base_subprocess),
                     ("asyncio.base_tasks", asyncio.base_tasks),
                     ("asyncio.constants", asyncio.constants),
                     ("asyncio.coroutines", asyncio.coroutines),
                     ("asyncio.events", asyncio.events),
                     ("asyncio.exceptions", asyncio.exceptions),
                     ("asyncio.format_helpers", asyncio.format_helpers),
                     ("asyncio.futures", asyncio.futures),
                     ("asyncio.locks", asyncio.locks),
                     ("asyncio.log", asyncio.log),
                     ("asyncio.mixins", asyncio.mixins),
                     ("asyncio.protocols", asyncio.protocols),
                     ("asyncio.queues", asyncio.queues),
                     ("asyncio.runners", asyncio.runners),
                     ("asyncio.selector_events", asyncio.selector_events),
                     ("asyncio.staggered", asyncio.staggered),
                     ("asyncio.streams", asyncio.streams),
                     ("asyncio.subprocess", asyncio.subprocess),
                     ("asyncio.taskgroups", asyncio.taskgroups),
                     ("asyncio.tasks", asyncio.tasks),
                     ("asyncio.threads", asyncio.threads),
                     ("asyncio.timeouts", asyncio.timeouts),
                     ("asyncio.transports", asyncio.transports),
                     ("asyncio.trsock", asyncio.trsock),
                     ("collections.abc", collections.abc),
                     ("concurrent.futures", concurrent.futures),
                     ("concurrent.futures._base", concurrent.futures._base),
                     ("concurrent.futures.process", concurrent.futures.process),
                     ("concurrent.futures.thread", concurrent.futures.thread),
                     ("ctypes._endian", ctypes._endian),
                     ("ctypes.util", ctypes.util),
                     ("ctypes.wintypes", ctypes.wintypes),
                     ("dbm.dumb", dbm.dumb),
                     ("email.base64mime", email.base64mime),
                     ("email.charset", email.charset),
                     ("email.contentmanager", email.contentmanager),
                     ("email.encoders", email.encoders),
                     ("email.errors", email.errors),
                     ("email.feedparser", email.feedparser),
                     ("email.generator", email.generator),
                     ("email.header", email.header),
                     ("email.headerregistry", email.headerregistry),
                     ("email.iterators", email.iterators),
                     ("email.message", email.message),
                     ("email.mime", email.mime),
                     ("email.mime.application", email.mime.application),
                     ("email.mime.audio", email.mime.audio),
                     ("email.mime.base", email.mime.base),
                     ("email.mime.image", email.mime.image),
                     ("email.mime.message", email.mime.message),
                     ("email.mime.multipart", email.mime.multipart),
                     ("email.mime.nonmultipart", email.mime.nonmultipart),
                     ("email.mime.text", email.mime.text),
                     ("email.parser", email.parser),
                     ("email.policy", email.policy),
                     ("email.quoprimime", email.quoprimime),
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
                     ("ensurepip.__main__", ensurepip.__main__),
                     ("ensurepip._uninstall", ensurepip._uninstall),
                     ("html.entities", html.entities),
                     ("html.parser", html.parser),
                     ("http.client", http.client),
                     ("http.cookiejar", http.cookiejar),
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
                     ("pydoc_data.topics", pydoc_data.topics),
                     ("json.decoder", json.decoder),
                     ("json.encoder", json.encoder),
                     ("json.scanner", json.scanner),
                     ("json.tool", json.tool),
                     ("logging.config", logging.config),
                     ("logging.handlers", logging.handlers),
                     ("multiprocessing.connection", multiprocessing.connection),
                     ("multiprocessing.context", multiprocessing.context),
                     ("multiprocessing.dummy", multiprocessing.dummy),
                     ("multiprocessing.dummy.connection", multiprocessing.dummy.connection),
                     ("multiprocessing.heap", multiprocessing.heap),
                     ("multiprocessing.managers", multiprocessing.managers),
                     ("multiprocessing.pool", multiprocessing.pool),
                     ("multiprocessing.process", multiprocessing.process),
                     ("multiprocessing.queues", multiprocessing.queues),
                     ("multiprocessing.reduction", multiprocessing.reduction),
                     ("multiprocessing.resource_sharer", multiprocessing.resource_sharer),
                     ("multiprocessing.resource_tracker", multiprocessing.resource_tracker),
                     ("multiprocessing.shared_memory", multiprocessing.shared_memory),
                     ("multiprocessing.sharedctypes", multiprocessing.sharedctypes),
                     ("multiprocessing.spawn", multiprocessing.spawn),
                     ("multiprocessing.synchronize", multiprocessing.synchronize),
                     ("multiprocessing.util", multiprocessing.util),
                     ("sqlite3.dbapi2", sqlite3.dbapi2),
                     ("sqlite3.dump", sqlite3.dump),
                     ("unittest.mock", unittest.mock),
                     ("urllib.error", urllib.error),
                     ("urllib.parse", urllib.parse),
                     ("urllib.request", urllib.request),
                     ("urllib.response", urllib.response),
                     ("urllib.robotparser", urllib.robotparser),
                     ("wsgiref.handlers", wsgiref.handlers),
                     ("wsgiref.headers", wsgiref.headers),
                     ("wsgiref.simple_server", wsgiref.simple_server),
                     ("wsgiref.types", wsgiref.types),
                     ("wsgiref.util", wsgiref.util),
                     ("wsgiref.validate", wsgiref.validate),
                     ("pyexpat.errors", pyexpat.errors),
                     ("pyexpat.model", pyexpat.model),
                     ("xml.dom", xml.dom),
                     ("xml.dom.NodeFilter", xml.dom.NodeFilter),
                     ("xml.dom.domreg", xml.dom.domreg),
                     ("xml.dom.expatbuilder", xml.dom.expatbuilder),
                     ("xml.dom.minicompat", xml.dom.minicompat),
                     ("xml.dom.minidom", xml.dom.minidom),
                     ("xml.dom.pulldom", xml.dom.pulldom),
                     ("xml.dom.xmlbuilder", xml.dom.xmlbuilder),
                     ("xml.etree", xml.etree),
                     ("xml.etree.ElementInclude", xml.etree.ElementInclude),
                     ("xml.etree.ElementPath", xml.etree.ElementPath),
                     ("xml.etree.ElementTree", xml.etree.ElementTree),
                     ("xml.etree.cElementTree", xml.etree.cElementTree),
                     ("xml.parsers", xml.parsers),
                     ("xml.parsers.expat", xml.parsers.expat),
                     ("xml.sax", xml.sax),
                     ("xml.sax.expatreader", xml.sax.expatreader),
                     ("xml.sax.handler", xml.sax.handler),
                     ("xml.sax.saxutils", xml.sax.saxutils),
                     ("xml.sax.xmlreader", xml.sax.xmlreader),
                     ("xmlrpc.client", xmlrpc.client),
                     ("xmlrpc.server", xmlrpc.server),
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

#[cfg(unix)]
const POSIX_PROBE: &str = r#"
import curses.ascii
import curses.has_key
import curses.panel
import curses.textpad
import multiprocessing.popen_fork
import multiprocessing.popen_forkserver
import multiprocessing.popen_spawn_posix
names = ["_curses", "_curses_panel", "curses", "fcntl", "grp", "posix", "pty", "pwd", "readline", "resource", "syslog", "termios", "tty"]
for name in names:
    print(name, list(vars(__import__(name))))
for name, module in [("curses.ascii", curses.ascii),
                     ("curses.has_key", curses.has_key),
                     ("curses.panel", curses.panel),
                     ("curses.textpad", curses.textpad),
                     ("multiprocessing.popen_fork", multiprocessing.popen_fork),
                     ("multiprocessing.popen_forkserver", multiprocessing.popen_forkserver),
                     ("multiprocessing.popen_spawn_posix", multiprocessing.popen_spawn_posix)]:
    print(name, list(vars(module)))
"#;

#[test]
fn builtin_module_dict_order_is_identical_across_processes() {
    assert_stable_across_runs("module_dict_order", PROBE);
}

#[cfg(unix)]
#[test]
fn posix_builtin_module_dict_order_is_identical_across_processes() {
    assert_stable_across_runs("posix_module_dict_order", POSIX_PROBE);
}

#[test]
fn star_import_binding_order_is_identical_across_processes() {
    assert_stable_across_runs("star_import_order", STAR_IMPORT_PROBE);
}

#[cfg(windows)]
const WINDOWS_PROBE: &str = r#"
import multiprocessing.popen_spawn_win32
for name in ["msvcrt", "nt", "winreg", "winsound"]:
    print(name, list(vars(__import__(name))))
print("multiprocessing.popen_spawn_win32", list(vars(multiprocessing.popen_spawn_win32)))
"#;

#[cfg(windows)]
#[test]
fn windows_builtin_module_dict_order_is_identical_across_processes() {
    assert_stable_across_runs("windows_module_dict_order", WINDOWS_PROBE);
}
