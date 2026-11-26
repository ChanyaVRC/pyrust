# Issue #3292: Verify CPython 3.12 parity for lazy stdlib package facades.
# Plain `import <pkg>` must not eagerly import/bind unrelated submodules,
# while CPython-eager bindings (`os.path`, `html.entities`, `encodings.aliases`,
# `pyexpat.errors`, `pyexpat.model`, `xml.dom.domreg`, `xml.sax.handler`,
# `xml.sax.xmlreader`, `importlib.resources.abc`) and explicit submodule imports
# must preserve exact module identity.

import sys

import xml
print("xml_before:", [a for a in ("etree", "dom", "sax", "parsers") if hasattr(xml, a)])
print("xml_mods_before:", [k for k in ("xml.etree", "xml.dom", "xml.sax", "xml.parsers") if k in sys.modules])

import http
print("http_before:", [a for a in ("client", "server", "cookies", "cookiejar") if hasattr(http, a)])
print("http_mods_before:", [k for k in ("http.client", "http.server", "http.cookies", "http.cookiejar") if k in sys.modules])

import urllib
print("urllib_before:", [a for a in ("parse", "request", "error", "response", "robotparser") if hasattr(urllib, a)])
print("urllib_mods_before:", [k for k in ("urllib.parse", "urllib.request", "urllib.error", "urllib.response", "urllib.robotparser") if k in sys.modules])

import importlib
print("importlib_before:", [a for a in ("machinery", "util", "abc", "resources", "metadata") if hasattr(importlib, a)])
print("importlib_mods_before:", [k for k in ("importlib.machinery", "importlib.util", "importlib.abc", "importlib.resources", "importlib.metadata") if k in sys.modules])

import html
print("html_before:", hasattr(html, "entities"), hasattr(html, "parser"), "html.entities" in sys.modules, "html.parser" in sys.modules)

import collections
print("collections_before:", hasattr(collections, "abc"), "collections.abc" in sys.modules)

import dbm
print("dbm_before:", hasattr(dbm, "dumb"), "dbm.dumb" in sys.modules)

import email
print("email_before:", [a for a in ("utils", "header", "policy", "message", "parser") if hasattr(email, a)])

import pydoc_data
print("pydoc_data_before:", hasattr(pydoc_data, "topics"), "pydoc_data.topics" in sys.modules)

import wsgiref
print("wsgiref_before:", [a for a in ("util", "headers", "handlers", "simple_server", "validate", "types") if hasattr(wsgiref, a)])

import xmlrpc
print("xmlrpc_before:", [a for a in ("client", "server") if hasattr(xmlrpc, a)])

# CPython-eager bindings
import os
print("os_path:", os.path is sys.modules["os.path"])

import encodings
print("encodings_aliases:", encodings.aliases is sys.modules["encodings.aliases"])

import pyexpat
print("pyexpat_sub:", pyexpat.errors is sys.modules["pyexpat.errors"], pyexpat.model is sys.modules["pyexpat.model"])

import xml.dom
print("xml_dom:", xml.dom.domreg is sys.modules["xml.dom.domreg"], hasattr(xml.dom, "minidom"), hasattr(xml.dom, "pulldom"))

import xml.sax
print("xml_sax:", xml.sax.handler is sys.modules["xml.sax.handler"], xml.sax.xmlreader is sys.modules["xml.sax.xmlreader"], hasattr(xml.sax, "saxutils"), hasattr(xml.sax, "expatreader"))

# Explicit submodule imports bind onto their parent packages
import xml.etree.ElementTree as ET
import urllib.parse
import http.client
import html.parser
import collections.abc
import importlib.util
from dbm import dumb
from email import utils as email_utils
from pydoc_data import topics
from wsgiref import util as wsgiref_util
from xmlrpc import client as xmlrpc_client

print(
    "after_identities:",
    xml.etree.ElementTree is ET is sys.modules["xml.etree.ElementTree"],
    urllib.parse is sys.modules["urllib.parse"],
    http.client is sys.modules["http.client"],
    html.parser is sys.modules["html.parser"],
    collections.abc is sys.modules["collections.abc"],
    importlib.util is sys.modules["importlib.util"],
    dbm.dumb is dumb is sys.modules["dbm.dumb"],
    email.utils is email_utils is sys.modules["email.utils"],
    pydoc_data.topics is topics is sys.modules["pydoc_data.topics"],
    wsgiref.util is wsgiref_util is sys.modules["wsgiref.util"],
    xmlrpc.client is xmlrpc_client is sys.modules["xmlrpc.client"],
)
