"""Create portable serialized representations of Python objects.

Pure-Python implementation of Python 3.12's ``pickle`` module (issue #3188),
wire-compatible with CPython 3.12 across protocols 0 through 5.
"""

from copyreg import (
    _extension_cache,
    _extension_registry,
    _inverted_registry,
    _reconstructor as _copyreg_reconstructor,
    __newobj__ as _copyreg_newobj,
    __newobj_ex__ as _copyreg_newobj_ex,
    dispatch_table as _dispatch_table,
)
from functools import partial
import io
from itertools import islice
from struct import pack, unpack
import sys
from types import BuiltinFunctionType, FunctionType

__all__ = [
    "PickleError",
    "PicklingError",
    "UnpicklingError",
    "Pickler",
    "Unpickler",
    "dump",
    "dumps",
    "load",
    "loads",
    "PickleBuffer",
]

# These are purely informational; no code uses these.
format_version = "4.0"  # File format version we write
compatible_formats = [
    "1.0",  # original protocol 0
    "1.1",  # protocol 0 with INST added
    "1.2",  # original protocol 1
    "1.3",  # protocol 1 with BINFLOAT added
    "2.0",  # protocol 2
    "3.0",  # protocol 3
    "4.0",  # protocol 4
    "5.0",  # protocol 5
]

# This is the highest protocol number we know how to read.
HIGHEST_PROTOCOL = 5

# The protocol we write by default.  May be less than HIGHEST_PROTOCOL.
# Matches CPython 3.12 (protocol 4).
DEFAULT_PROTOCOL = 4

bytes_types = (bytes, bytearray)

_maxsize = sys.maxsize


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class PickleError(Exception):
    """A common base class for the other pickling exceptions."""


class PicklingError(PickleError):
    """This exception is raised when an unpicklable object is passed to the
    dump() method.
    """


class UnpicklingError(PickleError):
    """This exception is raised when there is a problem unpickling an object,
    such as a security violation.
    """


class _Stop(Exception):
    def __init__(self, value):
        self.value = value


# ---------------------------------------------------------------------------
# PickleBuffer (protocol 5 out-of-band buffer support)
# ---------------------------------------------------------------------------


class PickleBuffer:
    """Wrapper for potentially out-of-band buffers (PEP 574 / protocol 5)."""

    def __init__(self, buffer):
        if isinstance(buffer, PickleBuffer):
            if buffer._released:
                raise ValueError("operation forbidden on released PickleBuffer object")
            self._buf = buffer._buf
            self._readonly = buffer._readonly
        elif isinstance(buffer, bytes):
            self._buf = buffer
            self._readonly = True
        elif isinstance(buffer, bytearray):
            self._buf = buffer
            self._readonly = False
        elif hasattr(buffer, "tobytes"):
            self._buf = buffer.tobytes()
            self._readonly = getattr(buffer, "readonly", True)
        else:
            raise TypeError(
                f"a bytes-like object is required, not '{type(buffer).__name__}'"
            )
        self._released = False

    def raw(self):
        if self._released:
            raise ValueError("operation forbidden on released PickleBuffer object")
        return self._buf

    def release(self):
        self._released = True
        self._buf = None

    def __bytes__(self):
        if self._released:
            raise ValueError("operation forbidden on released PickleBuffer object")
        return bytes(self._buf)


# ---------------------------------------------------------------------------
# Pickle opcodes
# ---------------------------------------------------------------------------

MARK = b"("  # push special markobject on stack
STOP = b"."  # every pickle ends with STOP
POP = b"0"  # discard topmost stack item
POP_MARK = b"1"  # discard stack top through topmost markobject
DUP = b"2"  # duplicate top stack item
FLOAT = b"F"  # push float object; decimal string argument
INT = b"I"  # push integer or bool; decimal string argument
BININT = b"J"  # push four-byte signed int
BININT1 = b"K"  # push 1-byte unsigned int
LONG = b"L"  # push long; decimal string argument
BININT2 = b"M"  # push 2-byte unsigned int
NONE = b"N"  # push None
PERSID = b"P"  # push persistent object; id is taken from string arg
BINPERSID = b"Q"  # "       "         "  ;  "  "   "     "  stack
REDUCE = b"R"  # apply callable to argtuple, both on stack
STRING = b"S"  # push string; NL-terminated string argument
BINSTRING = b"T"  # push string; counted binary string argument
SHORT_BINSTRING = b"U"  # "     "   ;    "      "       "      " < 256 bytes
UNICODE = b"V"  # push Unicode string; raw-unicode-escaped'd argument
BINUNICODE = b"X"  # "     "       "  ; counted UTF-8 string argument
APPEND = b"a"  # append stack top to list below it
BUILD = b"b"  # call __setstate__ or __dict__.update()
GLOBAL = b"c"  # push self.find_class(modname, name); 2 string args
DICT = b"d"  # build a dict from stack items
EMPTY_DICT = b"}"  # push empty dict
APPENDS = b"e"  # extend list on stack by topmost stack slice
GET = b"g"  # push item from memo on stack; index is string arg
BINGET = b"h"  # "    "    "    "   "   "  ;   "    " 1-byte arg
INST = b"i"  # build & push class instance
LONG_BINGET = b"j"  # push item from memo on stack; index is 4-byte arg
LIST = b"l"  # build list from topmost stack items
EMPTY_LIST = b"]"  # push empty list
OBJ = b"o"  # build & push class instance
PUT = b"p"  # store stack top in memo; index is string arg
BINPUT = b"q"  # "     "    "   "   " ;   "    " 1-byte arg
LONG_BINPUT = b"r"  # "     "    "   "   " ;   "    " 4-byte arg
SETITEM = b"s"  # add key+value pair to dict
TUPLE = b"t"  # build tuple from topmost stack items
EMPTY_TUPLE = b")"  # push empty tuple
SETITEMS = b"u"  # modify dict by adding topmost key+value pairs
BINFLOAT = b"G"  # push float; arg is 8-byte float encoding

TRUE = b"I01\n"  # not an opcode; see INT docs in pickletools.py
FALSE = b"I00\n"  # not an opcode; see INT docs in pickletools.py

# Protocol 2
PROTO = b"\x80"  # identify pickle protocol
NEWOBJ = b"\x81"  # build object by applying cls.__new__ to argtuple
EXT1 = b"\x82"  # push object from extension registry; 1-byte index
EXT2 = b"\x83"  # ditto, but 2-byte index
EXT4 = b"\x84"  # ditto, but 4-byte index
TUPLE1 = b"\x85"  # build 1-tuple from stack top
TUPLE2 = b"\x86"  # build 2-tuple from two topmost stack items
TUPLE3 = b"\x87"  # build 3-tuple from three topmost stack items
NEWTRUE = b"\x88"  # push True
NEWFALSE = b"\x89"  # push False
LONG1 = b"\x8a"  # push long from < 256 bytes
LONG4 = b"\x8b"  # push really big long

_tuplesize2code = [EMPTY_TUPLE, TUPLE1, TUPLE2, TUPLE3]

# Protocol 3 (Python 3.x)
BINBYTES = b"B"  # push bytes; counted binary string argument
SHORT_BINBYTES = b"C"  # "     "   ;    "      "       "      " < 256 bytes

# Protocol 4
SHORT_BINUNICODE = b"\x8c"  # push short string; UTF-8 length < 256 bytes
BINUNICODE8 = b"\x8d"  # push very long string
BINBYTES8 = b"\x8e"  # push very long bytes string
EMPTY_SET = b"\x8f"  # push empty set on the stack
ADDITEMS = b"\x90"  # modify set by adding topmost stack items
FROZENSET = b"\x91"  # build frozenset from topmost stack items
NEWOBJ_EX = b"\x92"  # like NEWOBJ but work with keyword only arguments
STACK_GLOBAL = b"\x93"  # same as GLOBAL but using names on the stacks
MEMOIZE = b"\x94"  # store top of the stack in memo
FRAME = b"\x95"  # indicate the beginning of a new frame

# Protocol 5
BYTEARRAY8 = b"\x96"  # push bytearray
NEXT_BUFFER = b"\x97"  # push next out-of-band buffer
READONLY_BUFFER = b"\x98"  # make top of stack readonly

_OPCODE_NAMES = [
    "ADDITEMS",
    "APPEND",
    "APPENDS",
    "BINBYTES",
    "BINBYTES8",
    "BINFLOAT",
    "BINGET",
    "BININT",
    "BININT1",
    "BININT2",
    "BINPERSID",
    "BINPUT",
    "BINSTRING",
    "BINUNICODE",
    "BINUNICODE8",
    "BUILD",
    "BYTEARRAY8",
    "DEFAULT_PROTOCOL",
    "DICT",
    "DUP",
    "EMPTY_DICT",
    "EMPTY_LIST",
    "EMPTY_SET",
    "EMPTY_TUPLE",
    "EXT1",
    "EXT2",
    "EXT4",
    "FALSE",
    "FLOAT",
    "FRAME",
    "FROZENSET",
    "GET",
    "GLOBAL",
    "HIGHEST_PROTOCOL",
    "INST",
    "INT",
    "LIST",
    "LONG",
    "LONG1",
    "LONG4",
    "LONG_BINGET",
    "LONG_BINPUT",
    "MARK",
    "MEMOIZE",
    "NEWFALSE",
    "NEWOBJ",
    "NEWOBJ_EX",
    "NEWTRUE",
    "NEXT_BUFFER",
    "NONE",
    "OBJ",
    "PERSID",
    "POP",
    "POP_MARK",
    "PROTO",
    "PUT",
    "READONLY_BUFFER",
    "REDUCE",
    "SETITEM",
    "SETITEMS",
    "SHORT_BINBYTES",
    "SHORT_BINSTRING",
    "SHORT_BINUNICODE",
    "STACK_GLOBAL",
    "STOP",
    "STRING",
    "TRUE",
    "TUPLE",
    "TUPLE1",
    "TUPLE2",
    "TUPLE3",
    "UNICODE",
]
__all__.extend(_OPCODE_NAMES)


# ---------------------------------------------------------------------------
# Python 2 <-> Python 3 compatibility mappings (from CPython _compat_pickle)
# ---------------------------------------------------------------------------

_IMPORT_MAPPING = {
    "__builtin__": "builtins",
    "copy_reg": "copyreg",
    "Queue": "queue",
    "SocketServer": "socketserver",
    "ConfigParser": "configparser",
    "repr": "reprlib",
    "tkFileDialog": "tkinter.filedialog",
    "tkSimpleDialog": "tkinter.simpledialog",
    "tkColorChooser": "tkinter.colorchooser",
    "tkCommonDialog": "tkinter.commondialog",
    "Dialog": "tkinter.dialog",
    "Tkdnd": "tkinter.dnd",
    "tkFont": "tkinter.font",
    "tkMessageBox": "tkinter.messagebox",
    "ScrolledText": "tkinter.scrolledtext",
    "Tkconstants": "tkinter.constants",
    "Tix": "tkinter.tix",
    "ttk": "tkinter.ttk",
    "Tkinter": "tkinter",
    "markupbase": "_markupbase",
    "_winreg": "winreg",
    "thread": "_thread",
    "dummy_thread": "_dummy_thread",
    "dbhash": "dbm.bsd",
    "dumbdbm": "dbm.dumb",
    "dbm": "dbm.ndbm",
    "gdbm": "dbm.gnu",
    "xmlrpclib": "xmlrpc.client",
    "SimpleXMLRPCServer": "xmlrpc.server",
    "httplib": "http.client",
    "htmlentitydefs": "html.entities",
    "HTMLParser": "html.parser",
    "Cookie": "http.cookies",
    "cookielib": "http.cookiejar",
    "BaseHTTPServer": "http.server",
    "test.test_support": "test.support",
    "commands": "subprocess",
    "urlparse": "urllib.parse",
    "robotparser": "urllib.robotparser",
    "urllib2": "urllib.request",
    "anydbm": "dbm",
    "_abcoll": "collections.abc",
}

_NAME_MAPPING = {
    ("__builtin__", "xrange"): ("builtins", "range"),
    ("__builtin__", "reduce"): ("functools", "reduce"),
    ("__builtin__", "intern"): ("sys", "intern"),
    ("__builtin__", "unichr"): ("builtins", "chr"),
    ("__builtin__", "unicode"): ("builtins", "str"),
    ("__builtin__", "long"): ("builtins", "int"),
    ("itertools", "izip"): ("builtins", "zip"),
    ("itertools", "imap"): ("builtins", "map"),
    ("itertools", "ifilter"): ("builtins", "filter"),
    ("itertools", "ifilterfalse"): ("itertools", "filterfalse"),
    ("itertools", "izip_longest"): ("itertools", "zip_longest"),
    ("UserDict", "IterableUserDict"): ("collections", "UserDict"),
    ("UserList", "UserList"): ("collections", "UserList"),
    ("UserString", "UserString"): ("collections", "UserString"),
    ("whichdb", "whichdb"): ("dbm", "whichdb"),
    ("_socket", "fromfd"): ("socket", "fromfd"),
    ("_multiprocessing", "Connection"): ("multiprocessing.connection", "Connection"),
    ("multiprocessing.process", "Process"): ("multiprocessing.context", "Process"),
    ("multiprocessing.forking", "Popen"): ("multiprocessing.popen_fork", "Popen"),
    ("urllib", "ContentTooShortError"): ("urllib.error", "ContentTooShortError"),
    ("urllib", "getproxies"): ("urllib.request", "getproxies"),
    ("urllib", "pathname2url"): ("urllib.request", "pathname2url"),
    ("urllib", "quote_plus"): ("urllib.parse", "quote_plus"),
    ("urllib", "quote"): ("urllib.parse", "quote"),
    ("urllib", "unquote_plus"): ("urllib.parse", "unquote_plus"),
    ("urllib", "unquote"): ("urllib.parse", "unquote"),
    ("urllib", "url2pathname"): ("urllib.request", "url2pathname"),
    ("urllib", "urlcleanup"): ("urllib.request", "urlcleanup"),
    ("urllib", "urlencode"): ("urllib.parse", "urlencode"),
    ("urllib", "urlopen"): ("urllib.request", "urlopen"),
    ("urllib", "urlretrieve"): ("urllib.request", "urlretrieve"),
    ("urllib2", "HTTPError"): ("urllib.error", "HTTPError"),
    ("urllib2", "URLError"): ("urllib.error", "URLError"),
}

_PYTHON2_EXCEPTIONS = (
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
)

for _exc in _PYTHON2_EXCEPTIONS:
    _NAME_MAPPING[("exceptions", _exc)] = ("builtins", _exc)

for _exc in ("AuthenticationError", "BufferTooShort", "ProcessError", "TimeoutError"):
    _NAME_MAPPING[("multiprocessing", _exc)] = ("multiprocessing.context", _exc)

_REVERSE_IMPORT_MAPPING = {v: k for k, v in _IMPORT_MAPPING.items()}
_REVERSE_NAME_MAPPING = {v: k for k, v in _NAME_MAPPING.items()}

_IMPORT_MAPPING.update(
    {
        "cPickle": "pickle",
        "_elementtree": "xml.etree.ElementTree",
        "FileDialog": "tkinter.filedialog",
        "SimpleDialog": "tkinter.simpledialog",
        "DocXMLRPCServer": "xmlrpc.server",
        "SimpleHTTPServer": "http.server",
        "CGIHTTPServer": "http.server",
        "UserDict": "collections",
        "UserList": "collections",
        "UserString": "collections",
        "whichdb": "dbm",
        "StringIO": "io",
        "cStringIO": "io",
    }
)

_REVERSE_IMPORT_MAPPING.update(
    {
        "_bz2": "bz2",
        "_dbm": "dbm",
        "_functools": "functools",
        "_gdbm": "gdbm",
        "_pickle": "pickle",
    }
)

_NAME_MAPPING.update(
    {
        ("__builtin__", "basestring"): ("builtins", "str"),
        ("exceptions", "StandardError"): ("builtins", "Exception"),
        ("UserDict", "UserDict"): ("collections", "UserDict"),
        ("socket", "_socketobject"): ("socket", "SocketType"),
    }
)

_REVERSE_NAME_MAPPING.update(
    {
        ("_functools", "reduce"): ("__builtin__", "reduce"),
        ("tkinter.filedialog", "FileDialog"): ("FileDialog", "FileDialog"),
        ("tkinter.filedialog", "LoadFileDialog"): ("FileDialog", "LoadFileDialog"),
        ("tkinter.filedialog", "SaveFileDialog"): ("FileDialog", "SaveFileDialog"),
        ("tkinter.simpledialog", "SimpleDialog"): ("SimpleDialog", "SimpleDialog"),
        ("xmlrpc.server", "ServerHTMLDoc"): ("DocXMLRPCServer", "ServerHTMLDoc"),
        ("xmlrpc.server", "XMLRPCDocGenerator"): (
            "DocXMLRPCServer",
            "XMLRPCDocGenerator",
        ),
        ("xmlrpc.server", "DocXMLRPCRequestHandler"): (
            "DocXMLRPCServer",
            "DocXMLRPCRequestHandler",
        ),
        ("xmlrpc.server", "DocXMLRPCServer"): ("DocXMLRPCServer", "DocXMLRPCServer"),
        ("xmlrpc.server", "DocCGIXMLRPCRequestHandler"): (
            "DocXMLRPCServer",
            "DocCGIXMLRPCRequestHandler",
        ),
        ("http.server", "SimpleHTTPRequestHandler"): (
            "SimpleHTTPServer",
            "SimpleHTTPRequestHandler",
        ),
        ("http.server", "CGIHTTPRequestHandler"): (
            "CGIHTTPServer",
            "CGIHTTPRequestHandler",
        ),
        ("_socket", "socket"): ("socket", "_socketobject"),
    }
)

for _exc in (
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
):
    _REVERSE_NAME_MAPPING[("builtins", _exc)] = ("exceptions", "OSError")

_REVERSE_NAME_MAPPING[("builtins", "ModuleNotFoundError")] = ("exceptions", "ImportError")


# ---------------------------------------------------------------------------
# Built-in fallback helpers (for _codecs.encode and native object.__reduce__
# helpers whose __module__ is None)
# ---------------------------------------------------------------------------


def _codecs_encode(obj, encoding="utf-8", errors="strict"):
    return obj.encode(encoding, errors)


_codecs_encode.__module__ = "_codecs"
_codecs_encode.__name__ = "encode"
_codecs_encode.__qualname__ = "encode"


# Detect the native copyreg._reconstructor / copyreg.__newobj__ returned by
# PyRust's object.__reduce__ / object.__reduce_ex__ (whose __module__ is None).
class _ProbeClass:
    pass


_NATIVE_COPYREG_RECONSTRUCTOR = _ProbeClass().__reduce__()[0]
_NATIVE_COPYREG_NEWOBJ = _ProbeClass().__reduce_ex__(2)[0]
del _ProbeClass

_SYNTHETIC_GLOBALS = {
    ("_codecs", "encode"): _codecs_encode,
    ("copyreg", "_reconstructor"): _NATIVE_COPYREG_RECONSTRUCTOR,
    ("copyreg", "__newobj__"): _NATIVE_COPYREG_NEWOBJ,
    ("copyreg", "__newobj_ex__"): _copyreg_newobj_ex,
}

_SYNTHETIC_GLOBAL_NAMES = {
    id(_codecs_encode): ("_codecs", "encode"),
    id(_copyreg_reconstructor): ("copyreg", "_reconstructor"),
    id(_copyreg_newobj): ("copyreg", "__newobj__"),
    id(_copyreg_newobj_ex): ("copyreg", "__newobj_ex__"),
    id(_NATIVE_COPYREG_RECONSTRUCTOR): ("copyreg", "_reconstructor"),
    id(_NATIVE_COPYREG_NEWOBJ): ("copyreg", "__newobj__"),
}


# ---------------------------------------------------------------------------
# Pure-Python escape_decode for Python 2 STRING opcode (codecs-free)
# ---------------------------------------------------------------------------

_SIMPLE_ESCAPES = {
    ord("\\"): ord("\\"),
    ord("'"): ord("'"),
    ord('"'): ord('"'),
    ord("a"): 0x07,
    ord("b"): 0x08,
    ord("f"): 0x0C,
    ord("n"): 0x0A,
    ord("r"): 0x0D,
    ord("t"): 0x09,
    ord("v"): 0x0B,
}


def _escape_decode(data):
    out = bytearray()
    i = 0
    n = len(data)
    while i < n:
        b = data[i]
        if b != 0x5C:  # '\\'
            out.append(b)
            i += 1
            continue
        i += 1
        if i >= n:
            raise ValueError("Trailing \\ in string")
        c = data[i]
        if c == 0x0A:  # line continuation
            i += 1
        elif c in _SIMPLE_ESCAPES:
            out.append(_SIMPLE_ESCAPES[c])
            i += 1
        elif c == ord("x"):
            if i + 2 >= n:
                raise ValueError("invalid \\x escape")
            out.append(int(data[i + 1 : i + 3], 16))
            i += 3
        elif 0x30 <= c <= 0x37:  # octal \0 .. \377
            end = i + 1
            while end < n and end < i + 3 and 0x30 <= data[end] <= 0x37:
                end += 1
            out.append(int(data[i:end], 8) & 0xFF)
            i = end
        else:
            out.append(0x5C)
            out.append(c)
            i += 1
    return bytes(out)


# ---------------------------------------------------------------------------
# Framing helpers (Protocol 4 & 5)
# ---------------------------------------------------------------------------


class _Framer:
    _FRAME_SIZE_MIN = 4
    _FRAME_SIZE_TARGET = 64 * 1024

    def __init__(self, file_write):
        self.file_write = file_write
        self.current_frame = None

    def start_framing(self):
        self.current_frame = io.BytesIO()

    def end_framing(self):
        if self.current_frame and self.current_frame.tell() > 0:
            self.commit_frame(force=True)
            self.current_frame = None

    def commit_frame(self, force=False):
        if self.current_frame:
            f = self.current_frame
            if f.tell() >= self._FRAME_SIZE_TARGET or force:
                data = f.getvalue()
                write = self.file_write
                if len(data) >= self._FRAME_SIZE_MIN:
                    write(FRAME + pack("<Q", len(data)))
                write(data)
                self.current_frame = io.BytesIO()

    def write(self, data):
        if isinstance(data, bytearray):
            data = bytes(data)
        if self.current_frame:
            return self.current_frame.write(data)
        return self.file_write(data)

    def write_large_bytes(self, header, payload):
        write = self.file_write
        if self.current_frame:
            self.commit_frame(force=True)
        write(header)
        if isinstance(payload, bytearray):
            payload = bytes(payload)
        write(payload)


class _Unframer:
    def __init__(self, file_read, file_readline, file_tell=None):
        self.file_read = file_read
        self.file_readline = file_readline
        self.current_frame = None

    def readinto(self, buf):
        n = len(buf)
        data = self.read(n)
        if len(data) < n:
            raise UnpicklingError("pickle data was truncated")
        buf[:] = data
        return n

    def read(self, n):
        if self.current_frame:
            data = self.current_frame.read(n)
            if not data and n != 0:
                self.current_frame = None
                return self.file_read(n)
            if len(data) < n:
                raise UnpicklingError("pickle exhausted before end of frame")
            return data
        return self.file_read(n)

    def read_exact(self, n):
        data = self.read(n)
        if len(data) < n:
            raise UnpicklingError("pickle data was truncated")
        return data

    def readline(self):
        if self.current_frame:
            data = self.current_frame.readline()
            if not data:
                self.current_frame = None
                return self._file_readline_checked()
            if data[-1] != 0x0A:
                raise UnpicklingError("pickle exhausted before end of frame")
            return data
        return self._file_readline_checked()

    def _file_readline_checked(self):
        data = self.file_readline()
        if not data or data[-1] != 0x0A:
            raise UnpicklingError("pickle data was truncated")
        return data

    def load_frame(self, frame_size):
        if self.current_frame and self.current_frame.read() != b"":
            raise UnpicklingError(
                "beginning of a new frame before end of current frame"
            )
        data = self.file_read(frame_size)
        if len(data) < frame_size:
            raise UnpicklingError("pickle data was truncated")
        self.current_frame = io.BytesIO(data)


# ---------------------------------------------------------------------------
# Tools used for pickling / unpickling
# ---------------------------------------------------------------------------


def _getattribute(obj, name):
    parent = None
    for subpath in name.split("."):
        if subpath == "<locals>":
            raise AttributeError(f"Can't get local attribute {name!r} on {obj!r}")
        try:
            parent = obj
            obj = getattr(obj, subpath)
        except AttributeError:
            raise AttributeError(
                f"Can't get attribute {name!r} on {obj!r}"
            ) from None
    return obj, parent


def whichmodule(obj, name):
    """Find the module an object belongs to."""
    module_name = getattr(obj, "__module__", None)
    if module_name is not None:
        return module_name
    for module_name, module in sys.modules.copy().items():
        if (
            module_name == "__main__"
            or module_name == "__mp_main__"
            or module is None
        ):
            continue
        try:
            if _getattribute(module, name)[0] is obj:
                return module_name
        except AttributeError:
            pass
    return "__main__"


def encode_long(x):
    """Encode an int to a two's complement little-endian binary string."""
    if x == 0:
        return b""
    nbytes = (x.bit_length() >> 3) + 1
    result = x.to_bytes(nbytes, byteorder="little", signed=True)
    if x < 0 and nbytes > 1:
        if result[-1] == 0xFF and (result[-2] & 0x80) != 0:
            result = result[:-1]
    return result


def decode_long(data):
    """Decode an int from a two's complement little-endian binary string."""
    return int.from_bytes(data, byteorder="little", signed=True)


def _decode_utf8_surrogatepass(data):
    """Decode UTF-8 bytes with CPython 'surrogatepass' semantics."""
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        pass
    out = []
    i = 0
    n = len(data)
    chunk_start = 0
    while i < n:
        if (
            data[i] == 0xED
            and i + 2 < n
            and 0xA0 <= data[i + 1] <= 0xBF
            and 0x80 <= data[i + 2] <= 0xBF
        ):
            if i > chunk_start:
                out.append(data[chunk_start:i].decode("utf-8"))
            cp = ((data[i] & 0x0F) << 12) | ((data[i + 1] & 0x3F) << 6) | (data[i + 2] & 0x3F)
            out.append(chr(cp))
            i += 3
            chunk_start = i
        else:
            i += 1
    if chunk_start < n:
        out.append(data[chunk_start:].decode("utf-8"))
    return "".join(out)


def _format_invalid_key(byte_val):
    if byte_val < 0x20 or byte_val >= 0x7F:
        return f"'\\x{byte_val:02x}'"
    ch = chr(byte_val)
    if ch == "'":
        return '"\'"'
    if ch == "\\":
        return "'\\\\'"
    return f"'{ch}'"


# ---------------------------------------------------------------------------
# Pickler
# ---------------------------------------------------------------------------


class _Pickler:
    def __init__(self, file, protocol=None, *, fix_imports=True, buffer_callback=None):
        if protocol is None:
            protocol = DEFAULT_PROTOCOL
        if not isinstance(protocol, int):
            raise TypeError(
                f"an integer is required (got type {type(protocol).__name__})"
            )
        if protocol < 0:
            protocol = HIGHEST_PROTOCOL
        elif not 0 <= protocol <= HIGHEST_PROTOCOL:
            raise ValueError("pickle protocol must be <= %d" % HIGHEST_PROTOCOL)
        if buffer_callback is not None:
            if protocol < 5:
                raise ValueError("buffer_callback needs protocol >= 5")
            if not callable(buffer_callback):
                raise TypeError("buffer_callback must be callable")
        self._buffer_callback = buffer_callback
        try:
            self._file_write = file.write
        except AttributeError:
            raise TypeError("file must have a 'write' attribute")
        self.framer = _Framer(self._file_write)
        self.write = self.framer.write
        self._write_large_bytes = self.framer.write_large_bytes
        self.memo = {}
        # CPython hands every global of one module the same ``__module__``
        # string object, so the id-keyed memo emits a BINGET for the repeats.
        # Canonicalise equal module names to reproduce those payloads.
        self._global_modules = {}
        self.proto = int(protocol)
        self.bin = protocol >= 1
        self.fast = 0
        self.fix_imports = fix_imports and protocol < 3

    def clear_memo(self):
        """Clears the pickler's memo."""
        self.memo.clear()
        self._global_modules.clear()

    def dump(self, obj):
        """Write a pickled representation of obj to the open file."""
        if not hasattr(self, "_file_write"):
            raise PicklingError(
                "Pickler.__init__() was not called by %s.__init__()"
                % (self.__class__.__name__,)
            )
        if self.proto >= 2:
            self.write(PROTO + pack("<B", self.proto))
        if self.proto >= 4:
            self.framer.start_framing()
        self.save(obj)
        self.write(STOP)
        self.framer.end_framing()

    def memoize(self, obj):
        """Store an object in the memo."""
        if self.fast:
            return
        idx = len(self.memo)
        self.write(self.put(idx))
        self.memo[id(obj)] = idx, obj

    def put(self, idx):
        if self.proto >= 4:
            return MEMOIZE
        elif self.bin:
            if idx < 256:
                return BINPUT + pack("<B", idx)
            else:
                return LONG_BINPUT + pack("<I", idx)
        else:
            return PUT + repr(idx).encode("ascii") + b"\n"

    def get(self, i):
        if self.bin:
            if i < 256:
                return BINGET + pack("<B", i)
            else:
                return LONG_BINGET + pack("<I", i)
        return GET + repr(i).encode("ascii") + b"\n"

    def _default_reduce_getnewargs(self, obj, t):
        """Handle __getnewargs_ex__ / __getnewargs__ for proto >= 2 when the
        class inherits object.__reduce_ex__ and object.__reduce__.
        """
        getnewargs_ex = getattr(t, "__getnewargs_ex__", None)
        if getnewargs_ex is not None:
            newargs_ex = getnewargs_ex(obj)
            if not isinstance(newargs_ex, tuple) or len(newargs_ex) != 2:
                raise TypeError(
                    f"__getnewargs_ex__ should return a tuple of length 2, "
                    f"not {type(newargs_ex).__name__}"
                )
            args, kwargs = newargs_ex
            if not isinstance(args, tuple):
                raise TypeError(
                    f"first item of the tuple returned by __getnewargs_ex__ "
                    f"must be a tuple, not {type(args).__name__}"
                )
            if not isinstance(kwargs, dict):
                raise TypeError(
                    f"second item of the tuple returned by __getnewargs_ex__ "
                    f"must be a dict, not {type(kwargs).__name__}"
                )
            state = obj.__getstate__()
            listiter = iter(obj) if isinstance(obj, list) else None
            dictiter = iter(obj.items()) if isinstance(obj, dict) else None
            if kwargs:
                return (_copyreg_newobj_ex, (t, args, kwargs), state, listiter, dictiter)
            return (_copyreg_newobj, (t,) + args, state, listiter, dictiter)

        getnewargs = getattr(t, "__getnewargs__", None)
        if getnewargs is not None:
            args = getnewargs(obj)
            if not isinstance(args, tuple):
                raise TypeError(
                    f"__getnewargs__ should return a tuple, not {type(args).__name__}"
                )
            state = obj.__getstate__()
            listiter = iter(obj) if isinstance(obj, list) else None
            dictiter = iter(obj.items()) if isinstance(obj, dict) else None
            return (_copyreg_newobj, (t,) + args, state, listiter, dictiter)

        return None

    def save(self, obj, save_persistent_id=True):
        self.framer.commit_frame()

        # Check for persistent id (defined by a subclass)
        pid = self.persistent_id(obj)
        if pid is not None and save_persistent_id:
            self.save_pers(pid)
            return

        # Check the memo
        x = self.memo.get(id(obj))
        if x is not None:
            self.write(self.get(x[0]))
            return

        rv = NotImplemented
        reduce = getattr(self, "reducer_override", None)
        if reduce is not None:
            rv = reduce(obj)

        if rv is NotImplemented:
            # Check the type dispatch table
            t = type(obj)
            f = self.dispatch.get(t)
            if f is not None:
                f(self, obj)
                return

            # Check private dispatch table if any
            dt = getattr(self, "dispatch_table", _dispatch_table)
            reduce = dt.get(t) if dt is not None else None
            if reduce is not None:
                rv = reduce(obj)
            else:
                # Check for a class with a custom metaclass; treat as regular class
                if issubclass(t, type):
                    self.save_global(obj)
                    return

                # Support __getnewargs_ex__ / __getnewargs__ for protocol >= 2
                # on instances inheriting default object.__reduce_ex__ / __reduce__,
                # and reject slotted classes without __dict__/__getstate__ for proto < 2.
                custom_gna = None
                uses_default_reduce = (
                    getattr(t, "__reduce_ex__", None) is object.__reduce_ex__
                    and getattr(t, "__reduce__", None) is object.__reduce__
                )
                if uses_default_reduce and self.proto < 2:
                    if (
                        getattr(t, "__getstate__", None) is object.__getstate__
                        and not hasattr(obj, "__dict__")
                        and any("__slots__" in c.__dict__ for c in t.__mro__)
                    ):
                        raise TypeError(
                            "a class that defines __slots__ without defining "
                            "__getstate__ cannot be pickled"
                        )
                elif uses_default_reduce and self.proto >= 2:
                    custom_gna = self._default_reduce_getnewargs(obj, t)

                if custom_gna is not None:
                    rv = custom_gna
                else:
                    # Check for a __reduce_ex__ method, fall back to __reduce__
                    reduce = getattr(obj, "__reduce_ex__", None)
                    if reduce is not None:
                        rv = reduce(self.proto)
                    else:
                        reduce = getattr(obj, "__reduce__", None)
                        if reduce is not None:
                            rv = reduce()
                        else:
                            raise PicklingError(
                                "Can't pickle %r object: %r" % (t.__name__, obj)
                            )
                    # CPython copyreg._reduce_ex (proto < 2) omits falsy state.
                    if (
                        uses_default_reduce
                        and self.proto < 2
                        and isinstance(rv, tuple)
                        and len(rv) == 3
                        and not rv[2]
                    ):
                        rv = rv[:2]

        # Check for string returned by reduce(), meaning "save as global"
        if isinstance(rv, str):
            self.save_global(obj, rv)
            return

        # Assert that reduce() returned a tuple
        if not isinstance(rv, tuple):
            raise PicklingError("%s must return string or tuple" % reduce)

        # Assert that it returned an appropriately sized tuple
        length = len(rv)
        if not (2 <= length <= 6):
            raise PicklingError(
                "Tuple returned by %s must have two to six elements" % reduce
            )

        # Save the reduce() output and finally memoize the object
        self.save_reduce(*rv, obj=obj)

    def persistent_id(self, obj):
        return None

    def save_pers(self, pid):
        if self.bin:
            self.save(pid, save_persistent_id=False)
            self.write(BINPERSID)
        else:
            s = str(pid)
            if "\n" in s:
                raise PicklingError(
                    "persistent IDs in protocol 0 cannot contain newlines"
                )
            try:
                self.write(PERSID + s.encode("ascii") + b"\n")
            except UnicodeEncodeError:
                raise PicklingError(
                    "persistent IDs in protocol 0 must be ASCII strings"
                )

    def save_reduce(
        self,
        func,
        args,
        state=None,
        listitems=None,
        dictitems=None,
        state_setter=None,
        *,
        obj=None,
    ):
        if not isinstance(args, tuple):
            raise PicklingError("args from save_reduce() must be a tuple")
        if not callable(func):
            raise PicklingError("func from save_reduce() must be callable")

        save = self.save
        write = self.write

        func_name = getattr(func, "__name__", "")
        if self.proto >= 2 and func_name == "__newobj_ex__":
            cls, args, kwargs = args
            if not hasattr(cls, "__new__"):
                raise PicklingError(
                    f"args[0] from {func_name} args has no __new__"
                )
            if obj is not None and cls is not obj.__class__:
                raise PicklingError(
                    f"args[0] from {func_name} args has the wrong class"
                )
            if self.proto >= 4:
                save(cls)
                save(args)
                save(kwargs)
                write(NEWOBJ_EX)
            else:
                func = partial(cls.__new__, cls, *args, **kwargs)
                save(func)
                save(())
                write(REDUCE)
        elif self.proto >= 2 and func_name == "__newobj__":
            cls = args[0]
            if not hasattr(cls, "__new__"):
                raise PicklingError("args[0] from __newobj__ args has no __new__")
            if obj is not None and cls is not obj.__class__:
                raise PicklingError(
                    "args[0] from __newobj__ args has the wrong class"
                )
            args = args[1:]
            save(cls)
            save(args)
            write(NEWOBJ)
        else:
            save(func)
            save(args)
            write(REDUCE)

        if obj is not None:
            if id(obj) in self.memo:
                write(POP + self.get(self.memo[id(obj)][0]))
            else:
                self.memoize(obj)

        if listitems is not None:
            self._batch_appends(listitems)

        if dictitems is not None:
            self._batch_setitems(dictitems)

        if state is not None:
            if state_setter is None:
                save(state)
                write(BUILD)
            else:
                save(state_setter)
                save(obj)
                save(state)
                write(TUPLE2)
                write(REDUCE)
                write(POP)

    # Dispatch methods

    dispatch = {}

    def save_none(self, obj):
        self.write(NONE)

    dispatch[type(None)] = save_none

    def save_bool(self, obj):
        if self.proto >= 2:
            self.write(NEWTRUE if obj else NEWFALSE)
        else:
            self.write(TRUE if obj else FALSE)

    dispatch[bool] = save_bool

    def save_long(self, obj):
        if self.bin:
            if obj >= 0:
                if obj <= 0xFF:
                    self.write(BININT1 + pack("<B", obj))
                    return
                if obj <= 0xFFFF:
                    self.write(BININT2 + pack("<H", obj))
                    return
            if -0x80000000 <= obj <= 0x7FFFFFFF:
                self.write(BININT + pack("<i", obj))
                return
        if self.proto >= 2:
            encoded = encode_long(obj)
            n = len(encoded)
            if n < 256:
                self.write(LONG1 + pack("<B", n) + encoded)
            else:
                self.write(LONG4 + pack("<i", n) + encoded)
            return
        if -0x80000000 <= obj <= 0x7FFFFFFF:
            self.write(INT + repr(obj).encode("ascii") + b"\n")
        else:
            self.write(LONG + repr(obj).encode("ascii") + b"L\n")

    dispatch[int] = save_long

    def save_float(self, obj):
        if self.bin:
            self.write(BINFLOAT + pack(">d", obj))
        else:
            self.write(FLOAT + repr(obj).encode("ascii") + b"\n")

    dispatch[float] = save_float

    def save_complex(self, obj):
        self.save_reduce(complex, (obj.real, obj.imag), obj=obj)

    dispatch[complex] = save_complex

    def save_slice(self, obj):
        self.save_reduce(slice, (obj.start, obj.stop, obj.step), obj=obj)

    dispatch[slice] = save_slice

    def save_bytes(self, obj):
        if self.proto < 3:
            if not obj:
                self.save_reduce(bytes, (), obj=obj)
            else:
                self.save_reduce(
                    _codecs_encode, (obj.decode("latin1"), "latin1"), obj=obj
                )
            return
        n = len(obj)
        if n <= 0xFF:
            self.write(SHORT_BINBYTES + pack("<B", n) + obj)
        elif n > 0xFFFFFFFF and self.proto >= 4:
            self._write_large_bytes(BINBYTES8 + pack("<Q", n), obj)
        elif n >= self.framer._FRAME_SIZE_TARGET:
            self._write_large_bytes(BINBYTES + pack("<I", n), obj)
        else:
            self.write(BINBYTES + pack("<I", n) + obj)
        self.memoize(obj)

    dispatch[bytes] = save_bytes

    def save_bytearray(self, obj):
        if self.proto < 5:
            if not obj:
                self.save_reduce(bytearray, (), obj=obj)
            else:
                self.save_reduce(bytearray, (bytes(obj),), obj=obj)
            return
        n = len(obj)
        data = bytes(obj)
        if n >= self.framer._FRAME_SIZE_TARGET:
            self._write_large_bytes(BYTEARRAY8 + pack("<Q", n), data)
        else:
            self.write(BYTEARRAY8 + pack("<Q", n) + data)
        self.memoize(obj)

    dispatch[bytearray] = save_bytearray

    def save_picklebuffer(self, obj):
        if self.proto < 5:
            raise PicklingError("PickleBuffer can only pickled with protocol >= 5")
        raw = obj.raw()
        in_band = True
        if self._buffer_callback is not None:
            in_band = bool(self._buffer_callback(obj))
        if in_band:
            if obj._readonly:
                self.save_bytes(bytes(raw))
            else:
                self.save_bytearray(bytearray(raw))
        else:
            self.write(NEXT_BUFFER)
            if obj._readonly:
                self.write(READONLY_BUFFER)

    dispatch[PickleBuffer] = save_picklebuffer

    def save_str(self, obj):
        if self.bin:
            encoded = obj.encode("utf-8", "surrogatepass")
            n = len(encoded)
            if n <= 0xFF and self.proto >= 4:
                self.write(SHORT_BINUNICODE + pack("<B", n) + encoded)
            elif n > 0xFFFFFFFF and self.proto >= 4:
                self._write_large_bytes(BINUNICODE8 + pack("<Q", n), encoded)
            elif n >= self.framer._FRAME_SIZE_TARGET:
                self._write_large_bytes(BINUNICODE + pack("<I", n), encoded)
            else:
                self.write(BINUNICODE + pack("<I", n) + encoded)
        else:
            tmp = obj.replace("\\", "\\u005c")
            tmp = tmp.replace("\0", "\\u0000")
            tmp = tmp.replace("\n", "\\u000a")
            tmp = tmp.replace("\r", "\\u000d")
            tmp = tmp.replace("\x1a", "\\u001a")  # EOF on DOS
            self.write(UNICODE + tmp.encode("raw-unicode-escape") + b"\n")
        self.memoize(obj)

    dispatch[str] = save_str

    def save_tuple(self, obj):
        if not obj:
            if self.bin:
                self.write(EMPTY_TUPLE)
            else:
                self.write(MARK + TUPLE)
            return

        n = len(obj)
        save = self.save
        memo = self.memo
        if n <= 3 and self.proto >= 2:
            for element in obj:
                save(element)
            if id(obj) in memo:
                get = self.get(memo[id(obj)][0])
                self.write(POP * n + get)
            else:
                self.write(_tuplesize2code[n])
                self.memoize(obj)
            return

        write = self.write
        write(MARK)
        for element in obj:
            save(element)

        if id(obj) in memo:
            get = self.get(memo[id(obj)][0])
            if self.bin:
                write(POP_MARK + get)
            else:
                write(POP * (n + 1) + get)
            return

        write(TUPLE)
        self.memoize(obj)

    dispatch[tuple] = save_tuple

    def save_list(self, obj):
        if self.bin:
            self.write(EMPTY_LIST)
        else:
            self.write(MARK + LIST)

        self.memoize(obj)
        self._batch_appends(obj)

    dispatch[list] = save_list

    _BATCHSIZE = 1000

    def _batch_appends(self, items):
        save = self.save
        write = self.write

        if not self.bin:
            for x in items:
                save(x)
                write(APPEND)
            return

        it = iter(items)
        while True:
            tmp = list(islice(it, self._BATCHSIZE))
            n = len(tmp)
            if n > 1:
                write(MARK)
                for x in tmp:
                    save(x)
                write(APPENDS)
            elif n:
                save(tmp[0])
                write(APPEND)
            if n < self._BATCHSIZE:
                return

    def save_dict(self, obj):
        if self.bin:
            self.write(EMPTY_DICT)
        else:
            self.write(MARK + DICT)

        self.memoize(obj)
        self._batch_setitems(obj.items())

    dispatch[dict] = save_dict

    def _batch_setitems(self, items):
        save = self.save
        write = self.write

        if not self.bin:
            for k, v in items:
                save(k)
                save(v)
                write(SETITEM)
            return

        it = iter(items)
        while True:
            tmp = list(islice(it, self._BATCHSIZE))
            n = len(tmp)
            if n > 1:
                write(MARK)
                for k, v in tmp:
                    save(k)
                    save(v)
                write(SETITEMS)
            elif n:
                k, v = tmp[0]
                save(k)
                save(v)
                write(SETITEM)
            if n < self._BATCHSIZE:
                return

    def save_set(self, obj):
        save = self.save
        write = self.write

        if self.proto < 4:
            self.save_reduce(set, (list(obj),), obj=obj)
            return

        write(EMPTY_SET)
        self.memoize(obj)

        it = iter(obj)
        while True:
            batch = list(islice(it, self._BATCHSIZE))
            n = len(batch)
            if n > 0:
                write(MARK)
                for item in batch:
                    save(item)
                write(ADDITEMS)
            if n < self._BATCHSIZE:
                return

    dispatch[set] = save_set

    def save_frozenset(self, obj):
        save = self.save
        write = self.write

        if self.proto < 4:
            self.save_reduce(frozenset, (list(obj),), obj=obj)
            return

        write(MARK)
        for item in obj:
            save(item)

        if id(obj) in self.memo:
            write(POP_MARK + self.get(self.memo[id(obj)][0]))
            return

        write(FROZENSET)
        self.memoize(obj)

    dispatch[frozenset] = save_frozenset

    def save_global(self, obj, name=None):
        write = self.write

        synthetic = _SYNTHETIC_GLOBAL_NAMES.get(id(obj))
        if synthetic is not None and name is None:
            module_name, name = synthetic
            module = None
            parent = None
        else:
            if name is None:
                name = getattr(obj, "__qualname__", None)
            if name is None:
                name = obj.__name__

            if "<locals>" in name.split("."):
                raise AttributeError(f"Can't pickle local object {name!r}")

            module_name = whichmodule(obj, name)
            try:
                __import__(module_name, level=0)
                module = sys.modules[module_name]
                obj2, parent = _getattribute(module, name)
            except (ImportError, KeyError, AttributeError):
                raise PicklingError(
                    "Can't pickle %r: it's not found as %s.%s"
                    % (obj, module_name, name)
                ) from None
            else:
                if obj2 is not obj:
                    raise PicklingError(
                        "Can't pickle %r: it's not the same object as %s.%s"
                        % (obj, module_name, name)
                    )

        if self.proto >= 2:
            code = _extension_registry.get((module_name, name))
            if code:
                if code <= 0xFF:
                    write(EXT1 + pack("<B", code))
                elif code <= 0xFFFF:
                    write(EXT2 + pack("<H", code))
                else:
                    write(EXT4 + pack("<i", code))
                return

        lastname = name.rpartition(".")[2]
        if parent is module:
            name = lastname

        if self.proto >= 4:
            self.save(self._global_modules.setdefault(module_name, module_name))
            self.save(name)
            write(STACK_GLOBAL)
        elif parent is not module:
            self.save_reduce(getattr, (parent, lastname))
        elif self.proto >= 3:
            write(
                GLOBAL
                + module_name.encode("utf-8")
                + b"\n"
                + name.encode("utf-8")
                + b"\n"
            )
        else:
            if self.fix_imports:
                if (module_name, name) in _REVERSE_NAME_MAPPING:
                    module_name, name = _REVERSE_NAME_MAPPING[(module_name, name)]
                elif module_name in _REVERSE_IMPORT_MAPPING:
                    module_name = _REVERSE_IMPORT_MAPPING[module_name]
            try:
                write(
                    GLOBAL
                    + module_name.encode("ascii")
                    + b"\n"
                    + name.encode("ascii")
                    + b"\n"
                )
            except UnicodeEncodeError:
                raise PicklingError(
                    "can't pickle global identifier '%s.%s' using "
                    "pickle protocol %i" % (module_name, name, self.proto)
                ) from None

        self.memoize(obj)

    def save_type(self, obj):
        if obj is type(None):
            return self.save_reduce(type, (None,), obj=obj)
        elif obj is type(NotImplemented):
            return self.save_reduce(type, (NotImplemented,), obj=obj)
        elif obj is type(...):
            return self.save_reduce(type, (...,), obj=obj)
        return self.save_global(obj)

    dispatch[FunctionType] = save_global
    dispatch[BuiltinFunctionType] = save_global
    dispatch[type] = save_type


# ---------------------------------------------------------------------------
# Unpickler
# ---------------------------------------------------------------------------


class _Unpickler:
    def __init__(
        self, file, *, fix_imports=True, encoding="ASCII", errors="strict", buffers=None
    ):
        try:
            self._file_readline = file.readline
            self._file_read = file.read
        except AttributeError:
            raise TypeError(
                "file must have 'read' and 'readline' attributes"
            )
        self._buffers = iter(buffers) if buffers is not None else None
        self.memo = {}
        self.encoding = encoding
        self.errors = errors
        self.proto = 0
        self.fix_imports = fix_imports

    def _pop(self):
        if not self.stack:
            raise UnpicklingError("unpickling stack underflow")
        return self.stack.pop()

    def _peek(self):
        if not self.stack:
            raise UnpicklingError("unpickling stack underflow")
        return self.stack[-1]

    def load(self):
        """Read a pickled object representation from the open file."""
        if not hasattr(self, "_file_read"):
            raise UnpicklingError(
                "Unpickler.__init__() was not called by %s.__init__()"
                % (self.__class__.__name__,)
            )
        self._unframer = _Unframer(self._file_read, self._file_readline)
        self.read = self._unframer.read
        self._read_exact = self._unframer.read_exact
        self.readinto = self._unframer.readinto
        self.readline = self._unframer.readline
        self.metastack = []
        self.stack = []
        self.append = self.stack.append
        self.proto = 0
        read = self.read
        dispatch = self.dispatch
        try:
            while True:
                key = read(1)
                if not key:
                    raise EOFError("Ran out of input")
                if not isinstance(key, bytes_types):
                    raise TypeError(
                        f"a bytes-like object is required, not '{type(key).__name__}'"
                    )
                opcode = key[0]
                handler = dispatch.get(opcode)
                if handler is None:
                    raise UnpicklingError(
                        f"invalid load key, {_format_invalid_key(opcode)}."
                    )
                handler(self)
        except _Stop as stopinst:
            return stopinst.value

    def pop_mark(self):
        if not self.metastack:
            raise UnpicklingError("unpickling stack underflow")
        items = self.stack
        self.stack = self.metastack.pop()
        self.append = self.stack.append
        return items

    def persistent_load(self, pid):
        raise UnpicklingError("unsupported persistent id encountered")

    dispatch = {}

    def load_proto(self):
        proto = self._read_exact(1)[0]
        if not 0 <= proto <= HIGHEST_PROTOCOL:
            raise ValueError("unsupported pickle protocol: %d" % proto)
        self.proto = proto

    dispatch[PROTO[0]] = load_proto

    def load_frame(self):
        (frame_size,) = unpack("<Q", self._read_exact(8))
        if frame_size > _maxsize:
            raise ValueError("frame size > sys.maxsize: %d" % frame_size)
        self._unframer.load_frame(frame_size)

    dispatch[FRAME[0]] = load_frame

    def load_persid(self):
        try:
            pid = self.readline()[:-1].decode("ascii")
        except UnicodeDecodeError:
            raise UnpicklingError(
                "persistent IDs in protocol 0 must be ASCII strings"
            )
        self.append(self.persistent_load(pid))

    dispatch[PERSID[0]] = load_persid

    def load_binpersid(self):
        pid = self._pop()
        self.append(self.persistent_load(pid))

    dispatch[BINPERSID[0]] = load_binpersid

    def load_none(self):
        self.append(None)

    dispatch[NONE[0]] = load_none

    def load_false(self):
        self.append(False)

    dispatch[NEWFALSE[0]] = load_false

    def load_true(self):
        self.append(True)

    dispatch[NEWTRUE[0]] = load_true

    def load_int(self):
        data = self.readline()
        if data == FALSE[1:]:
            val = False
        elif data == TRUE[1:]:
            val = True
        else:
            val = int(data, 0)
        self.append(val)

    dispatch[INT[0]] = load_int

    def load_binint(self):
        self.append(unpack("<i", self._read_exact(4))[0])

    dispatch[BININT[0]] = load_binint

    def load_binint1(self):
        self.append(self._read_exact(1)[0])

    dispatch[BININT1[0]] = load_binint1

    def load_binint2(self):
        self.append(unpack("<H", self._read_exact(2))[0])

    dispatch[BININT2[0]] = load_binint2

    def load_long(self):
        val = self.readline()[:-1]
        if val and val[-1] == 0x4C:  # b'L'[0]
            val = val[:-1]
        self.append(int(val, 0))

    dispatch[LONG[0]] = load_long

    def load_long1(self):
        n = self._read_exact(1)[0]
        data = self._read_exact(n)
        self.append(decode_long(data))

    dispatch[LONG1[0]] = load_long1

    def load_long4(self):
        (n,) = unpack("<i", self._read_exact(4))
        if n < 0:
            raise UnpicklingError("LONG pickle has negative byte count")
        data = self._read_exact(n)
        self.append(decode_long(data))

    dispatch[LONG4[0]] = load_long4

    def load_float(self):
        self.append(float(self.readline()[:-1]))

    dispatch[FLOAT[0]] = load_float

    def load_binfloat(self):
        self.append(unpack(">d", self._read_exact(8))[0])

    dispatch[BINFLOAT[0]] = load_binfloat

    def _decode_string(self, value):
        if self.encoding == "bytes":
            return value
        return value.decode(self.encoding, self.errors)

    def load_string(self):
        data = self.readline()[:-1]
        if len(data) >= 2 and data[0] == data[-1] and data[0] in (0x22, 0x27):
            data = data[1:-1]
        else:
            raise UnpicklingError("the STRING opcode argument must be quoted")
        self.append(self._decode_string(_escape_decode(data)))

    dispatch[STRING[0]] = load_string

    def load_binstring(self):
        (length,) = unpack("<i", self._read_exact(4))
        if length < 0:
            raise UnpicklingError("BINSTRING pickle has negative byte count")
        data = self._read_exact(length)
        self.append(self._decode_string(data))

    dispatch[BINSTRING[0]] = load_binstring

    def load_binbytes(self):
        (length,) = unpack("<I", self._read_exact(4))
        if length > _maxsize:
            raise UnpicklingError(
                "BINBYTES exceeds system's maximum size of %d bytes" % _maxsize
            )
        self.append(self._read_exact(length))

    dispatch[BINBYTES[0]] = load_binbytes

    def load_unicode(self):
        self.append(self.readline()[:-1].decode("raw-unicode-escape"))

    dispatch[UNICODE[0]] = load_unicode

    def load_binunicode(self):
        (length,) = unpack("<I", self._read_exact(4))
        if length > _maxsize:
            raise UnpicklingError(
                "BINUNICODE exceeds system's maximum size of %d bytes" % _maxsize
            )
        self.append(_decode_utf8_surrogatepass(self._read_exact(length)))

    dispatch[BINUNICODE[0]] = load_binunicode

    def load_binunicode8(self):
        (length,) = unpack("<Q", self._read_exact(8))
        if length > _maxsize:
            raise UnpicklingError(
                "BINUNICODE8 exceeds system's maximum size of %d bytes" % _maxsize
            )
        self.append(_decode_utf8_surrogatepass(self._read_exact(length)))

    dispatch[BINUNICODE8[0]] = load_binunicode8

    def load_binbytes8(self):
        (length,) = unpack("<Q", self._read_exact(8))
        if length > _maxsize:
            raise UnpicklingError(
                "BINBYTES8 exceeds system's maximum size of %d bytes" % _maxsize
            )
        self.append(self._read_exact(length))

    dispatch[BINBYTES8[0]] = load_binbytes8

    def load_bytearray8(self):
        (length,) = unpack("<Q", self._read_exact(8))
        if length > _maxsize:
            raise UnpicklingError(
                "BYTEARRAY8 exceeds system's maximum size of %d bytes" % _maxsize
            )
        b = bytearray(length)
        self.readinto(b)
        self.append(b)

    dispatch[BYTEARRAY8[0]] = load_bytearray8

    def load_next_buffer(self):
        if self._buffers is None:
            raise UnpicklingError(
                "pickle stream refers to out-of-band data "
                "but no *buffers* argument was given"
            )
        try:
            buf = next(self._buffers)
        except StopIteration:
            raise UnpicklingError("not enough out-of-band buffers")
        self.append(buf)

    dispatch[NEXT_BUFFER[0]] = load_next_buffer

    def load_readonly_buffer(self):
        buf = self._peek()
        if isinstance(buf, PickleBuffer):
            if not buf._readonly:
                self.stack[-1] = PickleBuffer(bytes(buf.raw()))
        elif isinstance(buf, bytearray):
            self.stack[-1] = bytes(buf)

    dispatch[READONLY_BUFFER[0]] = load_readonly_buffer

    def load_short_binstring(self):
        length = self._read_exact(1)[0]
        data = self._read_exact(length)
        self.append(self._decode_string(data))

    dispatch[SHORT_BINSTRING[0]] = load_short_binstring

    def load_short_binbytes(self):
        length = self._read_exact(1)[0]
        self.append(self._read_exact(length))

    dispatch[SHORT_BINBYTES[0]] = load_short_binbytes

    def load_short_binunicode(self):
        length = self._read_exact(1)[0]
        self.append(_decode_utf8_surrogatepass(self._read_exact(length)))

    dispatch[SHORT_BINUNICODE[0]] = load_short_binunicode

    def load_tuple(self):
        items = self.pop_mark()
        self.append(tuple(items))

    dispatch[TUPLE[0]] = load_tuple

    def load_empty_tuple(self):
        self.append(())

    dispatch[EMPTY_TUPLE[0]] = load_empty_tuple

    def load_tuple1(self):
        if not self.stack:
            raise UnpicklingError("unpickling stack underflow")
        self.stack[-1] = (self.stack[-1],)

    dispatch[TUPLE1[0]] = load_tuple1

    def load_tuple2(self):
        if len(self.stack) < 2:
            raise UnpicklingError("unpickling stack underflow")
        self.stack[-2:] = [(self.stack[-2], self.stack[-1])]

    dispatch[TUPLE2[0]] = load_tuple2

    def load_tuple3(self):
        if len(self.stack) < 3:
            raise UnpicklingError("unpickling stack underflow")
        self.stack[-3:] = [(self.stack[-3], self.stack[-2], self.stack[-1])]

    dispatch[TUPLE3[0]] = load_tuple3

    def load_empty_list(self):
        self.append([])

    dispatch[EMPTY_LIST[0]] = load_empty_list

    def load_empty_dictionary(self):
        self.append({})

    dispatch[EMPTY_DICT[0]] = load_empty_dictionary

    def load_empty_set(self):
        self.append(set())

    dispatch[EMPTY_SET[0]] = load_empty_set

    def load_frozenset(self):
        items = self.pop_mark()
        self.append(frozenset(items))

    dispatch[FROZENSET[0]] = load_frozenset

    def load_list(self):
        items = self.pop_mark()
        self.append(items)

    dispatch[LIST[0]] = load_list

    def load_dict(self):
        items = self.pop_mark()
        d = {items[i]: items[i + 1] for i in range(0, len(items), 2)}
        self.append(d)

    dispatch[DICT[0]] = load_dict

    def _instantiate(self, klass, args):
        if args or not isinstance(klass, type) or hasattr(klass, "__getinitargs__"):
            try:
                value = klass(*args)
            except TypeError as err:
                raise TypeError(
                    "in constructor for %s: %s" % (klass.__name__, str(err))
                ) from err
        else:
            value = klass.__new__(klass)
        self.append(value)

    def load_inst(self):
        module = self.readline()[:-1].decode("ascii")
        name = self.readline()[:-1].decode("ascii")
        klass = self.find_class(module, name)
        self._instantiate(klass, self.pop_mark())

    dispatch[INST[0]] = load_inst

    def load_obj(self):
        args = self.pop_mark()
        if not args:
            raise UnpicklingError("unpickling stack underflow")
        cls = args.pop(0)
        self._instantiate(cls, args)

    dispatch[OBJ[0]] = load_obj

    def load_newobj(self):
        args = self._pop()
        cls = self._pop()
        if isinstance(cls, type) and not isinstance(getattr(cls, "__new__", None), FunctionType):
            obj = _NATIVE_COPYREG_NEWOBJ(cls, *args)
        else:
            obj = cls.__new__(cls, *args)
        self.append(obj)

    dispatch[NEWOBJ[0]] = load_newobj

    def load_newobj_ex(self):
        kwargs = self._pop()
        args = self._pop()
        cls = self._pop()
        obj = cls.__new__(cls, *args, **kwargs)
        self.append(obj)

    dispatch[NEWOBJ_EX[0]] = load_newobj_ex

    def load_global(self):
        module = self.readline()[:-1].decode("utf-8")
        name = self.readline()[:-1].decode("utf-8")
        klass = self.find_class(module, name)
        self.append(klass)

    dispatch[GLOBAL[0]] = load_global

    def load_stack_global(self):
        name = self._pop()
        module = self._pop()
        if type(name) is not str or type(module) is not str:
            raise UnpicklingError("STACK_GLOBAL requires str")
        self.append(self.find_class(module, name))

    dispatch[STACK_GLOBAL[0]] = load_stack_global

    def load_ext1(self):
        code = self._read_exact(1)[0]
        self.get_extension(code)

    dispatch[EXT1[0]] = load_ext1

    def load_ext2(self):
        (code,) = unpack("<H", self._read_exact(2))
        self.get_extension(code)

    dispatch[EXT2[0]] = load_ext2

    def load_ext4(self):
        (code,) = unpack("<i", self._read_exact(4))
        self.get_extension(code)

    dispatch[EXT4[0]] = load_ext4

    def get_extension(self, code):
        nil = []
        obj = _extension_cache.get(code, nil)
        if obj is not nil:
            self.append(obj)
            return
        key = _inverted_registry.get(code)
        if not key:
            if code <= 0:
                raise UnpicklingError("EXT specifies code <= 0")
            raise ValueError("unregistered extension code %d" % code)
        obj = self.find_class(*key)
        _extension_cache[code] = obj
        self.append(obj)

    def find_class(self, module, name):
        if hasattr(sys, "audit"):
            sys.audit("pickle.find_class", module, name)
        if self.proto < 3 and self.fix_imports:
            if (module, name) in _NAME_MAPPING:
                module, name = _NAME_MAPPING[(module, name)]
            elif module in _IMPORT_MAPPING:
                module = _IMPORT_MAPPING[module]
        try:
            __import__(module, level=0)
        except ModuleNotFoundError:
            synthetic = _SYNTHETIC_GLOBALS.get((module, name))
            if synthetic is not None:
                return synthetic
            raise
        if self.proto >= 4:
            return _getattribute(sys.modules[module], name)[0]
        return getattr(sys.modules[module], name)

    def load_reduce(self):
        args = self._pop()
        if not self.stack:
            raise UnpicklingError("unpickling stack underflow")
        func = self.stack[-1]
        self.stack[-1] = func(*args)

    dispatch[REDUCE[0]] = load_reduce

    def load_pop(self):
        if self.stack:
            del self.stack[-1]
        else:
            self.pop_mark()

    dispatch[POP[0]] = load_pop

    def load_pop_mark(self):
        self.pop_mark()

    dispatch[POP_MARK[0]] = load_pop_mark

    def load_dup(self):
        self.append(self._peek())

    dispatch[DUP[0]] = load_dup

    def load_get(self):
        i = int(self.readline()[:-1])
        try:
            self.append(self.memo[i])
        except KeyError:
            raise UnpicklingError(f"Memo value not found at index {i}") from None

    dispatch[GET[0]] = load_get

    def load_binget(self):
        i = self._read_exact(1)[0]
        try:
            self.append(self.memo[i])
        except KeyError:
            raise UnpicklingError(f"Memo value not found at index {i}") from None

    dispatch[BINGET[0]] = load_binget

    def load_long_binget(self):
        (i,) = unpack("<I", self._read_exact(4))
        try:
            self.append(self.memo[i])
        except KeyError:
            raise UnpicklingError(f"Memo value not found at index {i}") from None

    dispatch[LONG_BINGET[0]] = load_long_binget

    def load_put(self):
        i = int(self.readline()[:-1])
        if i < 0:
            raise ValueError("negative PUT argument")
        self.memo[i] = self._peek()

    dispatch[PUT[0]] = load_put

    def load_binput(self):
        i = self._read_exact(1)[0]
        if i < 0:
            raise ValueError("negative BINPUT argument")
        self.memo[i] = self._peek()

    dispatch[BINPUT[0]] = load_binput

    def load_long_binput(self):
        (i,) = unpack("<I", self._read_exact(4))
        if i > _maxsize:
            raise ValueError("negative LONG_BINPUT argument")
        self.memo[i] = self._peek()

    dispatch[LONG_BINPUT[0]] = load_long_binput

    def load_memoize(self):
        memo = self.memo
        memo[len(memo)] = self._peek()

    dispatch[MEMOIZE[0]] = load_memoize

    def load_append(self):
        value = self._pop()
        list_obj = self._peek()
        list_obj.append(value)

    dispatch[APPEND[0]] = load_append

    def load_appends(self):
        items = self.pop_mark()
        list_obj = self._peek()
        try:
            extend = list_obj.extend
        except AttributeError:
            pass
        else:
            extend(items)
            return
        append = list_obj.append
        for item in items:
            append(item)

    dispatch[APPENDS[0]] = load_appends

    def load_setitem(self):
        value = self._pop()
        key = self._pop()
        dict_obj = self._peek()
        dict_obj[key] = value

    dispatch[SETITEM[0]] = load_setitem

    def load_setitems(self):
        items = self.pop_mark()
        dict_obj = self._peek()
        for i in range(0, len(items), 2):
            dict_obj[items[i]] = items[i + 1]

    dispatch[SETITEMS[0]] = load_setitems

    def load_additems(self):
        items = self.pop_mark()
        set_obj = self._peek()
        if isinstance(set_obj, set):
            set_obj.update(items)
        else:
            add = set_obj.add
            for item in items:
                add(item)

    dispatch[ADDITEMS[0]] = load_additems

    def load_build(self):
        state = self._pop()
        inst = self._peek()
        setstate = getattr(inst, "__setstate__", None)
        if setstate is not None:
            setstate(state)
            return
        slotstate = None
        if isinstance(state, tuple) and len(state) == 2:
            state, slotstate = state
        if state:
            inst_dict = inst.__dict__
            intern = getattr(sys, "intern", None)
            for k, v in state.items():
                if type(k) is str and intern is not None:
                    inst_dict[intern(k)] = v
                else:
                    inst_dict[k] = v
        if slotstate:
            for k, v in slotstate.items():
                setattr(inst, k, v)

    dispatch[BUILD[0]] = load_build

    def load_mark(self):
        self.metastack.append(self.stack)
        self.stack = []
        self.append = self.stack.append

    dispatch[MARK[0]] = load_mark

    def load_stop(self):
        value = self._pop()
        raise _Stop(value)

    dispatch[STOP[0]] = load_stop


# ---------------------------------------------------------------------------
# Public aliases & convenience functions
# ---------------------------------------------------------------------------

Pickler = _Pickler
Unpickler = _Unpickler


def _dump(obj, file, protocol=None, *, fix_imports=True, buffer_callback=None):
    _Pickler(
        file, protocol, fix_imports=fix_imports, buffer_callback=buffer_callback
    ).dump(obj)


def _dumps(obj, protocol=None, *, fix_imports=True, buffer_callback=None):
    f = io.BytesIO()
    _Pickler(
        f, protocol, fix_imports=fix_imports, buffer_callback=buffer_callback
    ).dump(obj)
    return f.getvalue()


def _load(file, *, fix_imports=True, encoding="ASCII", errors="strict", buffers=None):
    return _Unpickler(
        file,
        fix_imports=fix_imports,
        buffers=buffers,
        encoding=encoding,
        errors=errors,
    ).load()


def _loads(s, /, *, fix_imports=True, encoding="ASCII", errors="strict", buffers=None):
    if isinstance(s, bytes):
        data = s
    elif isinstance(s, bytearray):
        data = bytes(s)
    elif isinstance(s, PickleBuffer):
        data = bytes(s)
    elif hasattr(s, "tobytes"):
        data = s.tobytes()
    else:
        raise TypeError(
            f"a bytes-like object is required, not '{type(s).__name__}'"
        )
    file = io.BytesIO(data)
    return _Unpickler(
        file,
        fix_imports=fix_imports,
        buffers=buffers,
        encoding=encoding,
        errors=errors,
    ).load()


dump = _dump
dumps = _dumps
load = _load
loads = _loads
