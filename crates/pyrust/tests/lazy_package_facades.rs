//! Regression tests for lazy stdlib package facades (issue #3292).
//!
//! Each test spawns a fresh `pyrust -S -c ...` process so `sys.modules` starts
//! completely clean and verifies both:
//! 1. Importing a package facade does NOT pull in unrelated transitive
//!    submodules (matching CPython 3.12).
//! 2. Explicitly importing submodules (via `import pkg.submod`,
//!    `from pkg import submod`, `from pkg.submod import ...`, and
//!    `from pkg import *`) still works and binds exact identical module objects
//!    onto their parent packages.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

fn run_isolated(code: &str) -> String {
    let output = Command::new(pyrust_bin())
        .args(["-S", "-c", code])
        .output()
        .expect("run pyrust");
    assert!(
        output.status.success(),
        "pyrust -S -c failed (status {:?}):\ncode:\n{code}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

#[test]
fn lazy_package_facades_do_not_pull_transitive_submodules() {
    let cases = [
        (
            "xml",
            r#"
import sys, xml
for forbidden in ("xml.etree", "xml.etree.ElementTree", "xml.dom", "xml.dom.minidom",
                  "xml.sax", "xml.parsers", "xml.parsers.expat", "pyexpat"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
for attr in ("etree", "dom", "sax", "parsers"):
    assert not hasattr(xml, attr), f"xml unexpectedly has attr {attr}"
print("ok")
"#,
        ),
        (
            "http",
            r#"
import sys, http
for forbidden in ("http.client", "http.server", "http.cookies", "http.cookiejar",
                  "socket", "email", "urllib"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
for attr in ("client", "server", "cookies", "cookiejar"):
    assert not hasattr(http, attr), f"http unexpectedly has attr {attr}"
print("ok")
"#,
        ),
        (
            "urllib.parse",
            r#"
import sys, urllib.parse
assert urllib.parse is sys.modules["urllib.parse"]
for forbidden in ("urllib.request", "urllib.error", "urllib.response", "urllib.robotparser",
                  "http", "http.client", "email", "socket"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
for attr in ("request", "error", "response", "robotparser"):
    assert not hasattr(urllib, attr), f"urllib unexpectedly has attr {attr}"
print("ok")
"#,
        ),
        (
            "importlib",
            r#"
import sys, importlib
for forbidden in ("importlib.machinery", "importlib.util", "importlib.abc",
                  "importlib.resources", "importlib.metadata", "importlib.readers",
                  "importlib.simple"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
for attr in ("machinery", "util", "abc", "resources", "metadata", "readers", "simple"):
    assert not hasattr(importlib, attr), f"importlib unexpectedly has attr {attr}"
print("ok")
"#,
        ),
        (
            "html",
            r#"
import sys, html
assert "html.entities" in sys.modules
assert html.entities is sys.modules["html.entities"]
assert "html.parser" not in sys.modules
assert not hasattr(html, "parser")
print("ok")
"#,
        ),
        (
            "collections",
            r#"
import sys, collections
assert "collections.abc" not in sys.modules
assert not hasattr(collections, "abc")
assert "_collections_abc" in sys.modules
assert isinstance(collections.UserDict({"a": 1}), collections._collections_abc.MutableMapping) if hasattr(collections, "_collections_abc") else True
print("ok")
"#,
        ),
        (
            "dbm",
            r#"
import sys, dbm
assert "dbm.dumb" not in sys.modules
assert not hasattr(dbm, "dumb")
print("ok")
"#,
        ),
        (
            "email",
            r#"
import sys, email
for forbidden in ("email.utils", "email.header", "email.policy", "email.message",
                  "email.parser", "email.mime", "email.errors", "urllib"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
for attr in ("utils", "header", "policy", "message", "parser", "mime"):
    assert not hasattr(email, attr), f"email unexpectedly has attr {attr}"
print("ok")
"#,
        ),
        (
            "pydoc_data",
            r#"
import sys, pydoc_data
assert "pydoc_data.topics" not in sys.modules
assert not hasattr(pydoc_data, "topics")
print("ok")
"#,
        ),
        (
            "wsgiref.util",
            r#"
import sys, wsgiref.util
assert wsgiref.util is sys.modules["wsgiref.util"]
for forbidden in ("wsgiref.headers", "wsgiref.handlers", "wsgiref.simple_server",
                  "wsgiref.validate", "wsgiref.types", "http", "http.server", "socket"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
for attr in ("headers", "handlers", "simple_server", "validate", "types"):
    assert not hasattr(wsgiref, attr), f"wsgiref unexpectedly has attr {attr}"
print("ok")
"#,
        ),
        (
            "xmlrpc.client",
            r#"
import sys, xmlrpc.client
assert xmlrpc.client is sys.modules["xmlrpc.client"]
for forbidden in ("xmlrpc.server", "http.server"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
assert not hasattr(xmlrpc, "server")
print("ok")
"#,
        ),
        (
            "xml.dom",
            r#"
import sys, xml.dom
assert "xml.dom.domreg" in sys.modules
assert xml.dom.domreg is sys.modules["xml.dom.domreg"]
for forbidden in ("xml.dom.minidom", "xml.dom.pulldom", "xml.dom.expatbuilder",
                  "xml.dom.xmlbuilder", "xml.dom.NodeFilter", "xml.dom.minicompat"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
print("ok")
"#,
        ),
        (
            "xml.sax",
            r#"
import sys, xml.sax
assert "xml.sax.handler" in sys.modules
assert "xml.sax.xmlreader" in sys.modules
assert xml.sax.handler is sys.modules["xml.sax.handler"]
assert xml.sax.xmlreader is sys.modules["xml.sax.xmlreader"]
for forbidden in ("xml.sax.saxutils", "xml.sax.expatreader"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
print("ok")
"#,
        ),
        (
            "importlib.resources",
            r#"
import sys, importlib.resources
assert "importlib.resources.abc" in sys.modules
assert importlib.resources.abc is sys.modules["importlib.resources.abc"]
for forbidden in ("importlib.resources.readers", "importlib.resources.simple"):
    assert forbidden not in sys.modules, f"unexpectedly loaded {forbidden}"
print("ok")
"#,
        ),
    ];

    for (label, code) in cases {
        let out = run_isolated(code);
        assert_eq!(out.trim(), "ok", "lazy facade check failed for {label}");
    }
}

#[test]
fn explicit_submodule_imports_and_cpython_eager_bindings_preserve_identity() {
    let code = r#"
import sys

# CPython-eager submodule bindings
import os
assert os.path is sys.modules["os.path"]

import encodings
assert encodings.aliases is sys.modules["encodings.aliases"]

import pyexpat
assert pyexpat.errors is sys.modules["pyexpat.errors"]
assert pyexpat.model is sys.modules["pyexpat.model"]

# Explicit dotted imports bind onto parents with exact identity
import xml.etree.ElementTree as ET
assert sys.modules["xml"].etree is sys.modules["xml.etree"]
assert sys.modules["xml.etree"].ElementTree is ET is sys.modules["xml.etree.ElementTree"]
assert ET.fromstring("<a/>").tag == "a"

import urllib.parse
assert sys.modules["urllib"].parse is urllib.parse is sys.modules["urllib.parse"]
assert urllib.parse.quote("a b") == "a%20b"

import http.client
assert sys.modules["http"].client is http.client is sys.modules["http.client"]

import html.entities
import html.parser
assert sys.modules["html"].entities is html.entities is sys.modules["html.entities"]
assert sys.modules["html"].parser is html.parser is sys.modules["html.parser"]

import collections.abc
assert sys.modules["collections"].abc is collections.abc is sys.modules["collections.abc"]
assert issubclass(sys.modules["collections"].UserDict, collections.abc.MutableMapping)

import importlib.util
assert sys.modules["importlib"].util is importlib.util is sys.modules["importlib.util"]

# `from pkg import submod` lazily loads built-in submodules
from dbm import dumb
assert sys.modules["dbm"].dumb is dumb is sys.modules["dbm.dumb"]

from email import utils as email_utils
assert sys.modules["email"].utils is email_utils is sys.modules["email.utils"]

from pydoc_data import topics
assert sys.modules["pydoc_data"].topics is topics is sys.modules["pydoc_data.topics"]

from wsgiref import headers as wsgi_headers
assert sys.modules["wsgiref"].headers is wsgi_headers is sys.modules["wsgiref.headers"]

from xmlrpc import server as xmlrpc_server
assert sys.modules["xmlrpc"].server is xmlrpc_server is sys.modules["xmlrpc.server"]

# `__import__` with non-empty fromlist lazily loads built-in submodules
m = __import__("urllib", fromlist=["error"])
assert m.error is sys.modules["urllib.error"]

print("all_ok")
"#;
    let out = run_isolated(code);
    assert_eq!(out.trim(), "all_ok");
}
