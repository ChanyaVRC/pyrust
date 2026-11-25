# Comprehensive CPython 3.12 parity tests for all ~52 `_*` internal
# C-extension compatibility shims (issue #3250).

import importlib

# 1. Verify every single `_*` internal module imports cleanly
INTERNAL_MODULES = [
    "_thread",
    "_io",
    "_abc",
    "_collections",
    "_collections_abc",
    "_weakref",
    "_weakrefset",
    "_ast",
    "_struct",
    "_operator",
    "_functools",
    "_socket",
    "_signal",
    "_stat",
    "_csv",
    "_json",
    "_pickle",
    "_random",
    "_bisect",
    "_heapq",
    "_datetime",
    "_decimal",
    "_string",
    "_sre",
    "_warnings",
    "_codecs",
    "_locale",
    "_contextvars",
    "_queue",
    "_hashlib",
    "_md5",
    "_sha1",
    "_sha2",
    "_sha3",
    "_blake2",
    "_bz2",
    "_lzma",
    "_sqlite3",
    "_elementtree",
    "_asyncio",
    "_typing",
    "_opcode",
    "_tokenize",
    "_zoneinfo",
    "_uuid",
    "_statistics",
    "_imp",
    "_compression",
    "_compat_pickle",
    "_markupbase",
    "_strptime",
    "_threading_local",
]

imported = {}
for name in INTERNAL_MODULES:
    mod = importlib.import_module(name)
    imported[name] = mod
print("all imported count:", len(imported))

# 2. _thread
_thread = imported["_thread"]
lock = _thread.allocate_lock()
print("_thread lock:", lock.locked(), lock.acquire(), lock.locked())
lock.release()
print("_thread unlocked:", lock.locked(), isinstance(_thread.get_ident(), int), _thread.TIMEOUT_MAX > 0)
print("_thread error:", _thread.error is RuntimeError)
rl = _thread.RLock()
with rl:
    with rl:
        print("_thread rlock reentrant ok")
loc = _thread._local()
loc.val = 99
print("_thread _local:", loc.val)

# 3. _io
_io = imported["_io"]
sio = _io.StringIO("hello")
bio = _io.BytesIO(b"world")
print("_io:", sio.read(), bio.read(), _io.DEFAULT_BUFFER_SIZE > 0)

# 4. _abc
_abc = imported["_abc"]
print("_abc cache token:", isinstance(_abc.get_cache_token(), int))

# 5. _collections & _collections_abc
_collections = imported["_collections"]
_collections_abc = imported["_collections_abc"]
d = _collections.deque([1, 2, 3])
d.appendleft(0)
dd = _collections.defaultdict(list)
dd["a"].append(1)
counts = {}
_collections._count_elements(counts, ["x", "y", "x"])
print("_collections:", list(d), dict(dd), counts, isinstance([], _collections_abc.Sequence))

# 6. _weakref & _weakrefset
_weakref = imported["_weakref"]
_weakrefset = imported["_weakrefset"]


class Obj:
    pass


o = Obj()
r = _weakref.ref(o)
ws = _weakrefset.WeakSet([o])
print("_weakref:", r() is o, o in ws)

# 7. _ast, _struct, _operator, _functools
_ast = imported["_ast"]
_struct = imported["_struct"]
_operator = imported["_operator"]
_functools = imported["_functools"]
packed = _struct.pack(">HH", 1, 2)
print(
    "_ast/_struct/_operator/_functools:",
    issubclass(_ast.Module, _ast.AST),
    _struct.unpack(">HH", packed),
    _operator.add(3, 4),
    _operator._compare_digest(b"abc", b"abc"),
    _functools.reduce(_operator.add, [1, 2, 3, 4]),
    _functools.partial(_operator.add, 10)(5),
)

# 8. _socket, _signal, _stat, _csv
_socket = imported["_socket"]
_signal = imported["_signal"]
_stat = imported["_stat"]
_csv = imported["_csv"]
print(
    "_socket/_signal/_stat/_csv:",
    isinstance(_socket.AF_INET, int),
    isinstance(_signal.SIGINT, int),
    _stat.S_ISREG(0o100644),
    list(_csv.reader(["a,b", "1,2"])),
)

# 9. _json, _pickle, _random, _bisect, _heapq
_json = imported["_json"]
_pickle = imported["_pickle"]
_random = imported["_random"]
_bisect = imported["_bisect"]
_heapq = imported["_heapq"]
print("_json:", _json.encode_basestring("hi\n"), _json.encode_basestring_ascii("café"), _json.scanstring('"hello\\nworld"', 1))
print("_pickle:", _pickle.loads(_pickle.dumps({"a": [1, 2]})))
rng = _random.Random(42)
print("_random:", round(rng.random(), 6))
arr = [1, 3, 5, 7]
print("_bisect:", _bisect.bisect_left(arr, 4), _bisect.bisect_right(arr, 5))
h = [5, 1, 3]
_heapq.heapify(h)
print("_heapq:", _heapq.heappop(h), _heapq.heappop(h))

# 10. _datetime, _decimal, _string, _sre
_datetime = imported["_datetime"]
_decimal = imported["_decimal"]
_string = imported["_string"]
_sre = imported["_sre"]
print("_datetime:", _datetime.date(2024, 1, 15).isoformat(), _datetime.MINYEAR, _datetime.MAXYEAR)
print("_decimal:", str(_decimal.Decimal("1.5") + _decimal.Decimal("2.25")))
print("_string parser:", list(_string.formatter_parser("a{0!r:>5}b{{c}}")))
first, rest = _string.formatter_field_name_split("foo.bar[0].baz")
print("_string split:", first, list(rest))
print("_sre:", _sre.getcodesize() in (2, 4), _sre.ascii_tolower(65), _sre.ascii_iscased(65), _sre.ascii_iscased(48))

# 11. _warnings, _codecs, _locale, _contextvars, _queue
_warnings = imported["_warnings"]
_codecs = imported["_codecs"]
_locale = imported["_locale"]
_contextvars = imported["_contextvars"]
_queue = imported["_queue"]
print("_codecs:", _codecs.utf_8_encode("hi"), _codecs.ascii_decode(b"ok"))
print("_locale:", isinstance(_locale.LC_ALL, int), _locale.CHAR_MAX)
cv = _contextvars.ContextVar("x", default=7)
tok = cv.set(42)
val_set = cv.get()
cv.reset(tok)
print("_contextvars:", val_set, cv.get())
sq = _queue.SimpleQueue()
sq.put("item")
print("_queue:", sq.get(), issubclass(_queue.Empty, Exception))
print("_warnings:", isinstance(_warnings.filters, list))

# 12. _hashlib, _md5, _sha1, _sha2, _sha3, _blake2
_hashlib = imported["_hashlib"]
_md5 = imported["_md5"]
_sha1 = imported["_sha1"]
_sha2 = imported["_sha2"]
_sha3 = imported["_sha3"]
_blake2 = imported["_blake2"]
print("_md5:", _md5.md5(b"abc").hexdigest())
print("_sha1:", _sha1.sha1(b"abc").hexdigest())
print("_sha2:", _sha2.sha256(b"abc").hexdigest(), _sha2.sha512(b"abc").hexdigest()[:16])
print("_sha3:", _sha3.sha3_256(b"abc").hexdigest(), _sha3.shake_128(b"abc").hexdigest(8))
print("_blake2:", _blake2.blake2b(b"abc", digest_size=16).hexdigest(), _blake2.BLAKE2B_MAX_DIGEST_SIZE, _blake2.BLAKE2S_MAX_DIGEST_SIZE)
print("_hashlib:", _hashlib.openssl_sha256(b"abc").hexdigest(), _hashlib.hmac_digest(b"k", b"msg", "sha256").hex()[:16])

# 13. _bz2, _lzma, _sqlite3, _elementtree
_bz2 = imported["_bz2"]
_lzma = imported["_lzma"]
_sqlite3 = imported["_sqlite3"]
_elementtree = imported["_elementtree"]
c = _bz2.BZ2Compressor()
bz = c.compress(b"hello bz2") + c.flush()
print("_bz2:", _bz2.BZ2Decompressor().decompress(bz))
lc = _lzma.LZMACompressor()
lz = lc.compress(b"hello lzma") + lc.flush()
print("_lzma:", _lzma.LZMADecompressor().decompress(lz))
conn = _sqlite3.connect(":memory:")
print("_sqlite3:", conn.execute("SELECT 1 + 2").fetchone())
conn.close()
el = _elementtree.Element("root")
sub = _elementtree.SubElement(el, "child")
sub.text = "hi"
print("_elementtree:", el.tag, len(el), el[0].text)

# 14. _asyncio, _typing, _opcode, _tokenize, _zoneinfo, _uuid, _statistics
_asyncio = imported["_asyncio"]
_typing = imported["_typing"]
_opcode = imported["_opcode"]
_tokenize = imported["_tokenize"]
_zoneinfo = imported["_zoneinfo"]
_uuid = imported["_uuid"]
_statistics = imported["_statistics"]
tv = _typing.TypeVar("T")
print("_typing:", tv.__name__, _typing._idfunc(123))
print("_opcode:", isinstance(_opcode.stack_effect(100, 0), int))
print("_zoneinfo:", issubclass(_zoneinfo.ZoneInfo, _datetime.tzinfo), _zoneinfo.ZoneInfo.__name__)
print("_statistics inv_cdf:", round(_statistics._normal_dist_inv_cdf(0.5, 0.0, 1.0), 6), round(_statistics._normal_dist_inv_cdf(0.975, 0.0, 1.0), 4))
print("_uuid:", isinstance(_uuid.has_uuid_generate_time_safe, int))
print("_asyncio:", hasattr(_asyncio, "Future") or hasattr(_asyncio, "get_running_loop"))
print("_tokenize:", hasattr(_tokenize, "TokenizerIter") or hasattr(_tokenize, "generate_tokens"))

# 15. _imp
_imp = imported["_imp"]
print("_imp is_builtin:", _imp.is_builtin("sys"), _imp.is_builtin("builtins"), _imp.is_builtin("errno") != 0, _imp.is_builtin("no_such_mod_3250"))
print("_imp is_frozen:", _imp.is_frozen("no_such_mod_3250"))
print("_imp lock:", _imp.lock_held())
_imp.acquire_lock()
print("_imp lock acquired:", _imp.lock_held())
_imp.release_lock()
print("_imp lock released:", _imp.lock_held())
print("_imp ext_suffixes:", isinstance(_imp.extension_suffixes(), list))

# 16. _compression
_compression = imported["_compression"]
print("_compression:", _compression.BUFFER_SIZE > 0, issubclass(_compression.BaseStream, _io._BufferedIOBase), issubclass(_compression.DecompressReader, _io._RawIOBase))
dr = _compression.DecompressReader(_io.BytesIO(bz), _bz2.BZ2Decompressor)
print("_compression DecompressReader:", dr.read(), dr.tell())
dr.close()

# 17. _compat_pickle
_compat_pickle = imported["_compat_pickle"]
print(
    "_compat_pickle:",
    _compat_pickle.IMPORT_MAPPING["__builtin__"],
    _compat_pickle.IMPORT_MAPPING["Queue"],
    _compat_pickle.NAME_MAPPING[("__builtin__", "xrange")],
    _compat_pickle.REVERSE_IMPORT_MAPPING["builtins"],
    "ValueError" in _compat_pickle.PYTHON2_EXCEPTIONS,
)

# 18. _markupbase
_markupbase = imported["_markupbase"]
try:
    _markupbase.ParserBase()
except RuntimeError as e:
    print("_markupbase direct init:", str(e))


class MyParser(_markupbase.ParserBase):
    def __init__(self):
        self.reset()


mp = MyParser()
print("_markupbase subclass pos:", mp.getpos())

# 19. _strptime
_strptime = imported["_strptime"]
st = _strptime._strptime_time("2024-06-15", "%Y-%m-%d")
print("_strptime_time:", st.tm_year, st.tm_mon, st.tm_mday)
del st
dt = _strptime._strptime_datetime(_datetime.datetime, "2024-06-15 12:30", "%Y-%m-%d %H:%M")
print("_strptime_datetime:", dt.year, dt.month, dt.day, dt.hour, dt.minute)
lt = _strptime.LocaleTime()
print("_strptime LocaleTime:", lt.a_weekday[:3], lt.a_month[1:4])

# 20. _threading_local
_threading_local = imported["_threading_local"]
tl = _threading_local.local()
tl.answer = 42
print("_threading_local:", tl.answer)
del imported
