// `_compat_pickle` internal module — 100% native-Rust Python 3.12 pickle
// Python 2 <-> Python 3 compatibility tables (issue #3250).
//
// Reference: CPython 3.12 `Lib/_compat_pickle.py`

use pyrust_derive::pyrust_module;

use crate::value::{PyDict, PyKey, Value};

const IMPORT_PAIRS: &[(&str, &str)] = &[
    ("__builtin__", "builtins"),
    ("copy_reg", "copyreg"),
    ("Queue", "queue"),
    ("SocketServer", "socketserver"),
    ("ConfigParser", "configparser"),
    ("repr", "reprlib"),
    ("tkFileDialog", "tkinter.filedialog"),
    ("tkSimpleDialog", "tkinter.simpledialog"),
    ("tkColorChooser", "tkinter.colorchooser"),
    ("tkCommonDialog", "tkinter.commondialog"),
    ("Dialog", "tkinter.dialog"),
    ("Tkdnd", "tkinter.dnd"),
    ("tkFont", "tkinter.font"),
    ("tkMessageBox", "tkinter.messagebox"),
    ("ScrolledText", "tkinter.scrolledtext"),
    ("Tkconstants", "tkinter.constants"),
    ("Tix", "tkinter.tix"),
    ("ttk", "tkinter.ttk"),
    ("Tkinter", "tkinter"),
    ("markupbase", "_markupbase"),
    ("_winreg", "winreg"),
    ("thread", "_thread"),
    ("dummy_thread", "_dummy_thread"),
    ("dbhash", "dbm.bsd"),
    ("dumbdbm", "dbm.dumb"),
    ("dbm", "dbm.ndbm"),
    ("gdbm", "dbm.gnu"),
    ("xmlrpclib", "xmlrpc.client"),
    ("SimpleXMLRPCServer", "xmlrpc.server"),
    ("httplib", "http.client"),
    ("htmlentitydefs", "html.entities"),
    ("HTMLParser", "html.parser"),
    ("Cookie", "http.cookies"),
    ("cookielib", "http.cookiejar"),
    ("BaseHTTPServer", "http.server"),
    ("test.test_support", "test.support"),
    ("commands", "subprocess"),
    ("urlparse", "urllib.parse"),
    ("robotparser", "urllib.robotparser"),
    ("urllib2", "urllib.request"),
    ("anydbm", "dbm"),
    ("_abcoll", "collections.abc"),
];

const EXTRA_IMPORT_PAIRS: &[(&str, &str)] = &[
    ("cPickle", "pickle"),
    ("_elementtree", "xml.etree.ElementTree"),
    ("FileDialog", "tkinter.filedialog"),
    ("SimpleDialog", "tkinter.simpledialog"),
    ("DocXMLRPCServer", "xmlrpc.server"),
    ("SimpleHTTPServer", "http.server"),
    ("CGIHTTPServer", "http.server"),
    ("UserDict", "collections"),
    ("UserList", "collections"),
    ("UserString", "collections"),
    ("whichdb", "dbm"),
    ("StringIO", "io"),
    ("cStringIO", "io"),
];

const EXTRA_REVERSE_IMPORT_PAIRS: &[(&str, &str)] = &[
    ("_bz2", "bz2"),
    ("_dbm", "dbm"),
    ("_functools", "functools"),
    ("_gdbm", "gdbm"),
    ("_pickle", "pickle"),
];

type NamePair = ((&'static str, &'static str), (&'static str, &'static str));

const NAME_PAIRS: &[NamePair] = &[
    (("__builtin__", "xrange"), ("builtins", "range")),
    (("__builtin__", "reduce"), ("functools", "reduce")),
    (("__builtin__", "intern"), ("sys", "intern")),
    (("__builtin__", "unichr"), ("builtins", "chr")),
    (("__builtin__", "unicode"), ("builtins", "str")),
    (("__builtin__", "long"), ("builtins", "int")),
    (("itertools", "izip"), ("builtins", "zip")),
    (("itertools", "imap"), ("builtins", "map")),
    (("itertools", "ifilter"), ("builtins", "filter")),
    (("itertools", "ifilterfalse"), ("itertools", "filterfalse")),
    (("itertools", "izip_longest"), ("itertools", "zip_longest")),
    (
        ("UserDict", "IterableUserDict"),
        ("collections", "UserDict"),
    ),
    (("UserList", "UserList"), ("collections", "UserList")),
    (("UserString", "UserString"), ("collections", "UserString")),
    (("whichdb", "whichdb"), ("dbm", "whichdb")),
    (("_socket", "fromfd"), ("socket", "fromfd")),
    (
        ("_multiprocessing", "Connection"),
        ("multiprocessing.connection", "Connection"),
    ),
    (
        ("multiprocessing.process", "Process"),
        ("multiprocessing.context", "Process"),
    ),
    (
        ("multiprocessing.forking", "Popen"),
        ("multiprocessing.popen_fork", "Popen"),
    ),
    (
        ("urllib", "ContentTooShortError"),
        ("urllib.error", "ContentTooShortError"),
    ),
    (("urllib", "getproxies"), ("urllib.request", "getproxies")),
    (
        ("urllib", "pathname2url"),
        ("urllib.request", "pathname2url"),
    ),
    (("urllib", "quote_plus"), ("urllib.parse", "quote_plus")),
    (("urllib", "quote"), ("urllib.parse", "quote")),
    (("urllib", "unquote_plus"), ("urllib.parse", "unquote_plus")),
    (("urllib", "unquote"), ("urllib.parse", "unquote")),
    (
        ("urllib", "url2pathname"),
        ("urllib.request", "url2pathname"),
    ),
    (("urllib", "urlcleanup"), ("urllib.request", "urlcleanup")),
    (("urllib", "urlencode"), ("urllib.parse", "urlencode")),
    (("urllib", "urlopen"), ("urllib.request", "urlopen")),
    (("urllib", "urlretrieve"), ("urllib.request", "urlretrieve")),
    (("urllib2", "HTTPError"), ("urllib.error", "HTTPError")),
    (("urllib2", "URLError"), ("urllib.error", "URLError")),
];

const EXTRA_NAME_PAIRS: &[NamePair] = &[
    (("__builtin__", "basestring"), ("builtins", "str")),
    (("exceptions", "StandardError"), ("builtins", "Exception")),
    (("UserDict", "UserDict"), ("collections", "UserDict")),
    (("socket", "_socketobject"), ("socket", "SocketType")),
];

const EXTRA_REVERSE_NAME_PAIRS: &[NamePair] = &[
    (("_functools", "reduce"), ("__builtin__", "reduce")),
    (
        ("tkinter.filedialog", "FileDialog"),
        ("FileDialog", "FileDialog"),
    ),
    (
        ("tkinter.filedialog", "LoadFileDialog"),
        ("FileDialog", "LoadFileDialog"),
    ),
    (
        ("tkinter.filedialog", "SaveFileDialog"),
        ("FileDialog", "SaveFileDialog"),
    ),
    (
        ("tkinter.simpledialog", "SimpleDialog"),
        ("SimpleDialog", "SimpleDialog"),
    ),
    (
        ("xmlrpc.server", "ServerHTMLDoc"),
        ("DocXMLRPCServer", "ServerHTMLDoc"),
    ),
    (
        ("xmlrpc.server", "XMLRPCDocGenerator"),
        ("DocXMLRPCServer", "XMLRPCDocGenerator"),
    ),
    (
        ("xmlrpc.server", "DocXMLRPCRequestHandler"),
        ("DocXMLRPCServer", "DocXMLRPCRequestHandler"),
    ),
    (
        ("xmlrpc.server", "DocXMLRPCServer"),
        ("DocXMLRPCServer", "DocXMLRPCServer"),
    ),
    (
        ("xmlrpc.server", "DocCGIXMLRPCRequestHandler"),
        ("DocXMLRPCServer", "DocCGIXMLRPCRequestHandler"),
    ),
    (
        ("http.server", "SimpleHTTPRequestHandler"),
        ("SimpleHTTPServer", "SimpleHTTPRequestHandler"),
    ),
    (
        ("http.server", "CGIHTTPRequestHandler"),
        ("CGIHTTPServer", "CGIHTTPRequestHandler"),
    ),
    (("_socket", "socket"), ("socket", "_socketobject")),
];

const PYTHON2_EXCEPTIONS: &[&str] = &[
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BaseException",
    "BufferError",
    "BytesWarning",
    "DeprecationWarning",
    "EOFError",
    "EnvironmentError",
    "Exception",
    "FloatingPointError",
    "FutureWarning",
    "GeneratorExit",
    "IOError",
    "ImportError",
    "ImportWarning",
    "IndentationError",
    "IndexError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "NameError",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PendingDeprecationWarning",
    "ReferenceError",
    "RuntimeError",
    "RuntimeWarning",
    "StopIteration",
    "SyntaxError",
    "SyntaxWarning",
    "SystemError",
    "SystemExit",
    "TabError",
    "TypeError",
    "UnboundLocalError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeError",
    "UnicodeTranslateError",
    "UnicodeWarning",
    "UserWarning",
    "ValueError",
    "Warning",
    "ZeroDivisionError",
    #[cfg(windows)]
    "WindowsError",
];

const MULTIPROCESSING_EXCEPTIONS: &[&str] = &[
    "AuthenticationError",
    "BufferTooShort",
    "ProcessError",
    "TimeoutError",
];

const PYTHON3_OSERROR_EXCEPTIONS: &[&str] = &[
    "BrokenPipeError",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "FileExistsError",
    "FileNotFoundError",
    "InterruptedError",
    "IsADirectoryError",
    "NotADirectoryError",
    "PermissionError",
    "ProcessLookupError",
    "TimeoutError",
];

const PYTHON3_IMPORTERROR_EXCEPTIONS: &[&str] = &["ModuleNotFoundError"];

fn pair_key(a: &str, b: &str) -> PyKey {
    PyKey::Tuple(vec![PyKey::str_from(a), PyKey::str_from(b)].into())
}

fn pair_val(a: &str, b: &str) -> Value {
    Value::tuple(vec![Value::string(a), Value::string(b)])
}

fn str_tuple(items: &[&str]) -> Value {
    Value::tuple(items.iter().map(|s| Value::string(*s)).collect())
}

fn build_import_mapping() -> Value {
    let mut d = PyDict::default();
    for &(k, v) in IMPORT_PAIRS {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    for &(k, v) in EXTRA_IMPORT_PAIRS {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    Value::dict(d)
}

fn build_reverse_import_mapping() -> Value {
    let mut d = PyDict::default();
    for &(k, v) in IMPORT_PAIRS {
        d.insert(PyKey::str_from(v), Value::string(k));
    }
    for &(k, v) in EXTRA_REVERSE_IMPORT_PAIRS {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    Value::dict(d)
}

fn build_name_mapping() -> Value {
    let mut d = PyDict::default();
    for &((m1, n1), (m2, n2)) in NAME_PAIRS {
        d.insert(pair_key(m1, n1), pair_val(m2, n2));
    }
    for &exc in PYTHON2_EXCEPTIONS {
        d.insert(pair_key("exceptions", exc), pair_val("builtins", exc));
    }
    for &exc in MULTIPROCESSING_EXCEPTIONS {
        d.insert(
            pair_key("multiprocessing", exc),
            pair_val("multiprocessing.context", exc),
        );
    }
    for &((m1, n1), (m2, n2)) in EXTRA_NAME_PAIRS {
        d.insert(pair_key(m1, n1), pair_val(m2, n2));
    }
    Value::dict(d)
}

fn build_reverse_name_mapping() -> Value {
    let mut d = PyDict::default();
    for &((m1, n1), (m2, n2)) in NAME_PAIRS {
        d.insert(pair_key(m2, n2), pair_val(m1, n1));
    }
    for &exc in PYTHON2_EXCEPTIONS {
        d.insert(pair_key("builtins", exc), pair_val("exceptions", exc));
    }
    for &exc in MULTIPROCESSING_EXCEPTIONS {
        d.insert(
            pair_key("multiprocessing.context", exc),
            pair_val("multiprocessing", exc),
        );
    }
    for &((m1, n1), (m2, n2)) in EXTRA_REVERSE_NAME_PAIRS {
        d.insert(pair_key(m1, n1), pair_val(m2, n2));
    }
    for &exc in PYTHON3_OSERROR_EXCEPTIONS {
        d.insert(pair_key("builtins", exc), pair_val("exceptions", "OSError"));
    }
    for &exc in PYTHON3_IMPORTERROR_EXCEPTIONS {
        d.insert(
            pair_key("builtins", exc),
            pair_val("exceptions", "ImportError"),
        );
    }
    Value::dict(d)
}

pyrust_module! {
    constants {
        "IMPORT_MAPPING" => build_import_mapping(),
        "NAME_MAPPING" => build_name_mapping(),
        "PYTHON2_EXCEPTIONS" => str_tuple(PYTHON2_EXCEPTIONS),
        "MULTIPROCESSING_EXCEPTIONS" => str_tuple(MULTIPROCESSING_EXCEPTIONS),
        "REVERSE_IMPORT_MAPPING" => build_reverse_import_mapping(),
        "REVERSE_NAME_MAPPING" => build_reverse_name_mapping(),
        "PYTHON3_OSERROR_EXCEPTIONS" => str_tuple(PYTHON3_OSERROR_EXCEPTIONS),
        "PYTHON3_IMPORTERROR_EXCEPTIONS" => str_tuple(PYTHON3_IMPORTERROR_EXCEPTIONS),
    }
}
