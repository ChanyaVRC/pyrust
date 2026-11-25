"""Python 3.12 DB-API 2.0 interface for SQLite databases."""

import collections.abc
import datetime
import os
import time
from warnings import warn

__package__ = "sqlite3"

# ── Module globals & constants ───────────────────────────────────────────────

apilevel = "2.0"
threadsafety = _threadsafety()
paramstyle = "qmark"

sqlite_version = _sqlite_version()
sqlite_version_info = tuple(int(x) for x in sqlite_version.split("."))

PARSE_DECLTYPES = 1
PARSE_COLNAMES = 2
LEGACY_TRANSACTION_CONTROL = -1

_CONSTANTS = _sqlite_constants()
for _cname, _cval in _CONSTANTS.items():
    globals()[_cname] = _cval
del _cname, _cval

# ── DB-API 2.0 Exception Hierarchy ───────────────────────────────────────────


class Warning(Exception):
    pass


class Error(Exception):
    pass


class InterfaceError(Error):
    pass


class DatabaseError(Error):
    pass


class DataError(DatabaseError):
    pass


class OperationalError(DatabaseError):
    pass


class IntegrityError(DatabaseError):
    pass


class InternalError(DatabaseError):
    pass


class ProgrammingError(DatabaseError):
    pass


class NotSupportedError(DatabaseError):
    pass


# ── DB-API 2.0 Types & Constructors ──────────────────────────────────────────

Date = datetime.date
Time = datetime.time
Timestamp = datetime.datetime


def DateFromTicks(ticks):
    return Date(*time.localtime(ticks)[:3])


def TimeFromTicks(ticks):
    return Time(*time.localtime(ticks)[3:6])


def TimestampFromTicks(ticks):
    return Timestamp(*time.localtime(ticks)[:6])


Binary = memoryview


# ── Adaptation & Conversion Registry ─────────────────────────────────────────


class PrepareProtocol:
    pass


adapters = {}
converters = {}

_NO_ALT = object()


def register_adapter(type_, adapter, /):
    """Register an adapter callable to adapt Python objects of type_ into SQLite types."""
    if not isinstance(type_, type):
        raise TypeError("register_adapter() argument 1 must be a type")
    if not callable(adapter):
        raise TypeError("register_adapter() argument 2 must be callable")
    if type_ in (int, float, str, bytes):
        # CPython allows registering adapters for subclasses, or builtins;
        # store directly in adapters dict.
        pass
    adapters[(type_, PrepareProtocol)] = adapter


def register_converter(typename, converter, /):
    """Register a converter callable to convert SQLite bytes to a Python object."""
    if not isinstance(typename, str):
        raise TypeError("register_converter() argument 1 must be str")
    if not callable(converter):
        raise TypeError("register_converter() argument 2 must be callable")
    converters[typename.upper()] = converter


def adapt(obj, protocol=PrepareProtocol, alt=_NO_ALT, /):
    """Adapt obj to protocol using registered adapters or __conform__."""
    # 1. Check exact type in adapters registry
    key = (type(obj), protocol)
    adapter = adapters.get(key)
    if adapter is not None:
        return adapter(obj)

    # 2. Check __conform__ protocol
    conform = getattr(obj, "__conform__", None)
    if conform is not None:
        res = conform(protocol)
        if res is not None:
            return res

    # 3. Fallback to alt if provided
    if alt is not _NO_ALT:
        return alt

    raise ProgrammingError("can't adapt")


def _register_default_adapters_and_converters():
    msg = (
        "The default {what} is deprecated as of Python 3.12; "
        "see the sqlite3 documentation for suggested replacement recipes"
    )

    def adapt_date(val):
        warn(msg.format(what="date adapter"), DeprecationWarning, stacklevel=2)
        return val.isoformat()

    def adapt_datetime(val):
        warn(msg.format(what="datetime adapter"), DeprecationWarning, stacklevel=2)
        return val.isoformat(" ")

    def convert_date(val):
        warn(msg.format(what="date converter"), DeprecationWarning, stacklevel=2)
        return datetime.date(*map(int, val.split(b"-")))

    def convert_timestamp(val):
        warn(msg.format(what="timestamp converter"), DeprecationWarning, stacklevel=2)
        datepart, timepart = val.split(b" ")
        year, month, day = map(int, datepart.split(b"-"))
        timepart_full = timepart.split(b".")
        hours, minutes, seconds = map(int, timepart_full[0].split(b":"))
        if len(timepart_full) == 2:
            microseconds = int("{:0<6.6}".format(timepart_full[1].decode()))
        else:
            microseconds = 0
        return datetime.datetime(year, month, day, hours, minutes, seconds, microseconds)

    register_adapter(datetime.date, adapt_date)
    register_adapter(datetime.datetime, adapt_datetime)
    register_converter("date", convert_date)
    register_converter("timestamp", convert_timestamp)


_register_default_adapters_and_converters()
del _register_default_adapters_and_converters


_NATIVE_SQLITE_TYPES = (type(None), bool, int, float, str, bytes, bytearray, memoryview)


def _adapt_param(val, index):
    """Adapt a single parameter value for SQLite binding."""
    # Exact builtin check first (fast path; preserves subclasses with registered adapters)
    t = type(val)
    if t in _NATIVE_SQLITE_TYPES:
        return val
    # Check adapter registry and __conform__
    adapter = adapters.get((t, PrepareProtocol))
    if adapter is not None:
        adapted = adapter(val)
        if isinstance(adapted, _NATIVE_SQLITE_TYPES):
            return adapted
        raise ProgrammingError(
            f"Error binding parameter {index}: type '{type(adapted).__name__}' is not supported"
        )
    conform = getattr(val, "__conform__", None)
    if conform is not None:
        adapted = conform(PrepareProtocol)
        if adapted is not None:
            if isinstance(adapted, _NATIVE_SQLITE_TYPES):
                return adapted
            raise ProgrammingError(
                f"Error binding parameter {index}: type '{type(adapted).__name__}' is not supported"
            )
    # Subclasses of native types (e.g., IntEnum, str subclass) without custom adapters
    if isinstance(val, bool):
        return bool(val)
    if isinstance(val, int):
        return int(val)
    if isinstance(val, float):
        return float(val)
    if isinstance(val, str):
        return str(val)
    if isinstance(val, (bytes, bytearray, memoryview)):
        return bytes(val)
    raise ProgrammingError(
        f"Error binding parameter {index}: type '{t.__name__}' is not supported"
    )


# ── Module-level functions ───────────────────────────────────────────────────


def complete_statement(statement):
    """Check if a string contains a complete SQL statement."""
    if not isinstance(statement, str):
        raise TypeError(
            f"complete_statement() argument 1 must be str, not {type(statement).__name__}"
        )
    return _complete_statement(statement)


def enable_callback_tracebacks(flag, /):
    """Enable or disable callback functions throwing errors to stderr."""
    _enable_callback_tracebacks(bool(flag))


# ── Helper: isolation_level validation & column converter resolution ─────────

_VALID_ISOLATION_LEVELS = ("", "DEFERRED", "IMMEDIATE", "EXCLUSIVE")


def _validate_isolation_level(level):
    if level is None:
        return None
    if not isinstance(level, str):
        raise TypeError(
            f"isolation_level must be str or None, not {type(level).__name__}"
        )
    upper = level.upper()
    if upper not in _VALID_ISOLATION_LEVELS:
        raise ValueError(
            "invalid value for isolation_level; expected '', 'DEFERRED', 'IMMEDIATE', 'EXCLUSIVE', or None"
        )
    return level


def _validate_autocommit(value):
    if value is LEGACY_TRANSACTION_CONTROL or (
        type(value) is int and value == LEGACY_TRANSACTION_CONTROL
    ):
        return LEGACY_TRANSACTION_CONTROL
    if type(value) is bool:
        return value
    raise ValueError(
        "autocommit must be True, False, or sqlite3.LEGACY_TRANSACTION_CONTROL"
    )


def _resolve_description_and_converters(col_meta, detect_types):
    """Build DB-API description tuple and per-column converter list."""
    if not col_meta:
        return None, []
    desc = []
    col_convs = []
    use_colnames = bool(detect_types & PARSE_COLNAMES)
    use_decltypes = bool(detect_types & PARSE_DECLTYPES)

    for col_name, decltype in col_meta:
        conv = None
        out_name = col_name

        if use_colnames and "[" in col_name and "]" in col_name:
            lbr = col_name.find("[")
            rbr = col_name.find("]", lbr + 1)
            if lbr != -1 and rbr != -1:
                typename = col_name[lbr + 1 : rbr].strip().upper()
                conv = converters.get(typename)
                # Strip trailing space + [typename] from reported column name
                out_name = col_name[:lbr].rstrip()

        if conv is None and use_decltypes and decltype:
            # First word before whitespace or '('
            part = decltype.strip()
            cut = len(part)
            for ch in (" ", "\t", "("):
                pos = part.find(ch)
                if pos != -1 and pos < cut:
                    cut = pos
            typename = part[:cut].upper()
            conv = converters.get(typename)

        desc.append((out_name, None, None, None, None, None, None))
        col_convs.append(conv)

    return tuple(desc), col_convs


def _encode_text_factory(tf):
    """Translate Connection.text_factory into the native helper's fast-path tag."""
    if tf is str:
        return None
    if tf is bytes:
        return True
    return tf


# ── Row ──────────────────────────────────────────────────────────────────────


class Row(collections.abc.Sequence):
    """Row factory supporting both index/slice and case-insensitive column-name lookup."""

    def __init__(self, cursor, data):
        if not isinstance(cursor, Cursor):
            raise TypeError(
                f"Row() argument 1 must be sqlite3.Cursor, not {type(cursor).__name__}"
            )
        if not isinstance(data, tuple):
            data = tuple(data)
        self._data = data
        desc = cursor.description
        if desc is not None:
            keys = tuple(col[0] for col in desc)
        else:
            keys = ()
        self._keys = keys
        keymap = {}
        for idx, k in enumerate(keys):
            kl = k.lower()
            if kl not in keymap:
                keymap[kl] = idx
        self._keymap = keymap

    def keys(self):
        return list(self._keys)

    def __len__(self):
        return len(self._data)

    def __iter__(self):
        return iter(self._data)

    def __getitem__(self, key):
        if isinstance(key, (int, slice)):
            return self._data[key]
        if isinstance(key, str):
            idx = self._keymap.get(key.lower())
            if idx is None:
                raise IndexError("No item with that key")
            return self._data[idx]
        raise IndexError("Index must be int or string")

    def __eq__(self, other):
        if not isinstance(other, Row):
            return NotImplemented
        return self._keys == other._keys and self._data == other._data

    def __ne__(self, other):
        if not isinstance(other, Row):
            return NotImplemented
        return not (self._keys == other._keys and self._data == other._data)

    def __hash__(self):
        return hash((self._keys, self._data))

    def __repr__(self):
        return f"<sqlite3.Row object at {hex(id(self))}>"


# ── Blob ─────────────────────────────────────────────────────────────────────


class Blob:
    """Incremental I/O interface to a SQLite BLOB value."""

    def __init__(self, handle):
        self._handle = handle
        self._closed = False
        self._offset = 0

    def _check_open(self):
        if self._closed or self._handle is None:
            raise ProgrammingError("Cannot operate on a closed blob.")

    def close(self):
        if not self._closed and self._handle is not None:
            _blob_close(self._handle)
            self._handle = None
            self._closed = True

    def __len__(self):
        self._check_open()
        return _blob_len(self._handle)

    def tell(self):
        self._check_open()
        return self._offset

    def seek(self, offset, origin=0, /):
        self._check_open()
        if not isinstance(offset, int) or isinstance(offset, bool):
            raise TypeError(f"offset must be int, not {type(offset).__name__}")
        length = _blob_len(self._handle)
        if origin == 0:  # os.SEEK_SET
            new_pos = offset
        elif origin == 1:  # os.SEEK_CUR
            new_pos = self._offset + offset
        elif origin == 2:  # os.SEEK_END
            new_pos = length + offset
        else:
            raise ValueError(
                "'origin' should be os.SEEK_SET, os.SEEK_CUR, or os.SEEK_END"
            )
        if new_pos < 0 or new_pos > length:
            raise ValueError("offset out of blob range")
        self._offset = new_pos

    def read(self, length=-1, /):
        self._check_open()
        if length is None:
            length = -1
        if not isinstance(length, int) or isinstance(length, bool):
            raise TypeError(f"length must be int, not {type(length).__name__}")
        total = _blob_len(self._handle)
        avail = total - self._offset
        if avail <= 0:
            return b""
        if length < 0 or length > avail:
            to_read = avail
        else:
            to_read = length
        if to_read == 0:
            return b""
        data = _blob_read(self._handle, self._offset, to_read)
        self._offset += len(data)
        return data

    def write(self, data, /):
        self._check_open()
        if isinstance(data, memoryview):
            raw = data.tobytes()
        elif isinstance(data, (bytes, bytearray)):
            raw = bytes(data)
        else:
            raise TypeError(
                f"a bytes-like object is required, not '{type(data).__name__}'"
            )
        total = _blob_len(self._handle)
        if self._offset + len(raw) > total:
            raise ValueError("data longer than blob length")
        _blob_write(self._handle, self._offset, raw)
        self._offset += len(raw)

    def __getitem__(self, key):
        self._check_open()
        total = _blob_len(self._handle)
        if isinstance(key, slice):
            start, stop, step = key.indices(total)
            if step == 1:
                if stop <= start:
                    return b""
                return _blob_read(self._handle, start, stop - start)
            full = _blob_read(self._handle, 0, total)
            return full[key]
        if isinstance(key, int) and not isinstance(key, bool):
            idx = key if key >= 0 else total + key
            if idx < 0 or idx >= total:
                raise IndexError("Blob index out of range")
            return _blob_read(self._handle, idx, 1)[0]
        raise TypeError("Blob indices must be integers or slices")

    def __setitem__(self, key, value):
        self._check_open()
        total = _blob_len(self._handle)
        if isinstance(key, slice):
            if isinstance(value, memoryview):
                raw = value.tobytes()
            elif isinstance(value, (bytes, bytearray)):
                raw = bytes(value)
            else:
                raise TypeError(
                    f"a bytes-like object is required, not '{type(value).__name__}'"
                )
            start, stop, step = key.indices(total)
            indices = range(start, stop, step)
            if len(raw) != len(indices):
                raise ValueError("Blob slice assignment is wrong size")
            if step == 1:
                if len(raw) > 0:
                    _blob_write(self._handle, start, raw)
            else:
                for idx, byte_val in zip(indices, raw):
                    _blob_write(self._handle, idx, bytes((byte_val,)))
            return
        if isinstance(key, int) and not isinstance(key, bool):
            idx = key if key >= 0 else total + key
            if idx < 0 or idx >= total:
                raise IndexError("Blob index out of range")
            if not isinstance(value, int) or isinstance(value, bool):
                raise TypeError(
                    f"'{type(value).__name__}' object cannot be interpreted as an integer"
                )
            if value < 0 or value > 255:
                raise ValueError("byte must be in range(0, 256)")
            _blob_write(self._handle, idx, bytes((value,)))
            return
        raise TypeError("Blob indices must be integers or slices")

    def __enter__(self):
        self._check_open()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


# ── iterdump (CPython 3.12 Lib/sqlite3/dump.py) ──────────────────────────────


def _iterdump(connection):
    writeable_schema = False
    cu = connection.cursor()
    # Always use tuple rows internally regardless of connection.row_factory
    cu.row_factory = None
    yield "BEGIN TRANSACTION;"

    q = """
        SELECT "name", "type", "sql"
        FROM "sqlite_master"
            WHERE "sql" NOT NULL AND
            "type" == 'table'
            ORDER BY "name"
        """
    schema_res = cu.execute(q)
    sqlite_sequence = []
    for table_name, _type, sql in schema_res.fetchall():
        if table_name == "sqlite_sequence":
            rows = cu.execute('SELECT * FROM "sqlite_sequence";').fetchall()
            sqlite_sequence = ['DELETE FROM "sqlite_sequence"']
            sqlite_sequence += [
                f"INSERT INTO \"sqlite_sequence\" VALUES('{row[0]}',{row[1]})"
                for row in rows
            ]
            continue
        elif table_name == "sqlite_stat1":
            yield 'ANALYZE "sqlite_master";'
        elif table_name.startswith("sqlite_"):
            continue
        elif sql.startswith("CREATE VIRTUAL TABLE"):
            if not writeable_schema:
                writeable_schema = True
                yield "PRAGMA writable_schema=ON;"
            yield (
                "INSERT INTO sqlite_master(type,name,tbl_name,rootpage,sql)"
                "VALUES('table','{0}','{0}',0,'{1}');".format(
                    table_name.replace("'", "''"),
                    sql.replace("'", "''"),
                )
            )
        else:
            yield "{0};".format(sql)

        table_name_ident = table_name.replace('"', '""')
        res = cu.execute('PRAGMA table_info("{0}")'.format(table_name_ident))
        column_names = [str(table_info[1]) for table_info in res.fetchall()]
        q = """SELECT 'INSERT INTO "{0}" VALUES({1})' FROM "{0}";""".format(
            table_name_ident,
            ",".join(
                """'||quote("{0}")||'""".format(col.replace('"', '""'))
                for col in column_names
            ),
        )
        query_res = cu.execute(q)
        for row in query_res:
            yield "{0};".format(row[0])

    q = """
        SELECT "name", "type", "sql"
        FROM "sqlite_master"
            WHERE "sql" NOT NULL AND
            "type" IN ('index', 'trigger', 'view')
        """
    schema_res = cu.execute(q)
    for _name, _type, sql in schema_res.fetchall():
        yield "{0};".format(sql)

    if writeable_schema:
        yield "PRAGMA writable_schema=OFF;"

    for row in sqlite_sequence:
        yield "{0};".format(row)

    yield "COMMIT;"


# ── Cursor ───────────────────────────────────────────────────────────────────


class Cursor:
    """SQLite database cursor."""

    def __init__(self, connection):
        if not isinstance(connection, Connection):
            raise TypeError(
                f"Cursor() argument 1 must be sqlite3.Connection, not {type(connection).__name__}"
            )
        self._connection = connection
        self._closed = False
        self._stmt = None
        self._description = None
        self._col_converters = []
        self._rowcount = -1
        self._lastrowid = None
        self._arraysize = 1
        self._row_factory = connection.row_factory
        self._pending_row = None
        self._has_pending = False
        self._exhausted = True

    def _check_open(self):
        if self._closed:
            raise ProgrammingError("Cannot operate on a closed cursor.")
        self._connection._check_open()

    def _close_stmt(self):
        if self._stmt is not None:
            _stmt_close(self._stmt)
            self._stmt = None
        self._pending_row = None
        self._has_pending = False
        self._exhausted = True

    @property
    def connection(self):
        return self._connection

    @property
    def description(self):
        return self._description

    @property
    def rowcount(self):
        return self._rowcount

    @property
    def lastrowid(self):
        return self._lastrowid

    @property
    def arraysize(self):
        return self._arraysize

    @arraysize.setter
    def arraysize(self, value):
        if not isinstance(value, int) or isinstance(value, bool):
            raise TypeError(f"arraysize must be int, not {type(value).__name__}")
        self._arraysize = value

    @property
    def row_factory(self):
        return self._row_factory

    @row_factory.setter
    def row_factory(self, value):
        self._row_factory = value

    def _resolve_params(self, parameters, param_count, param_names):
        if isinstance(parameters, dict):
            bound = []
            for idx, pname in enumerate(param_names):
                if pname is None:
                    raise ProgrammingError(
                        f"Binding {idx + 1} has no name, but you supplied a dictionary (which has only names)."
                    )
                key = pname[1:]  # strip leading ':', '@', or '$'
                if key not in parameters:
                    raise ProgrammingError(
                        f"You did not supply a value for binding parameter {pname}."
                    )
                bound.append(_adapt_param(parameters[key], idx + 1))
            return bound

        # Positional sequence / iterable
        if isinstance(parameters, (tuple, list)):
            seq = parameters
        elif isinstance(parameters, (str, bytes, bytearray)):
            raise ProgrammingError(
                f"parameters are of unsupported type"
            )
        else:
            try:
                seq = list(parameters)
            except TypeError:
                raise ProgrammingError("parameters are of unsupported type")

        if len(seq) != param_count:
            raise ProgrammingError(
                f"Incorrect number of bindings supplied. The current statement uses {param_count}, and there are {len(seq)} supplied."
            )
        return [_adapt_param(v, idx + 1) for idx, v in enumerate(seq)]

    def _format_row(self, raw_row):
        rf = self._row_factory
        if rf is None:
            return raw_row
        return rf(self, raw_row)

    def execute(self, sql, parameters=(), /):
        self._check_open()
        if not isinstance(sql, str):
            raise TypeError(
                f"execute() argument 1 must be str, not {type(sql).__name__}"
            )
        self._close_stmt()

        stmt, is_dml, col_count, col_meta, param_count, param_names = _stmt_prepare(
            self._connection._handle, sql
        )
        if stmt is None:
            self._description = None
            self._col_converters = []
            self._rowcount = -1
            return self

        self._stmt = stmt
        try:
            # Legacy implicit transaction on DML statements
            if (
                is_dml
                and self._connection._autocommit == LEGACY_TRANSACTION_CONTROL
                and self._connection._isolation_level is not None
                and not self._connection.in_transaction
            ):
                begin_sql = (
                    f"BEGIN {self._connection._isolation_level}".rstrip()
                )
                _conn_exec_simple(self._connection._handle, begin_sql)

            bound = self._resolve_params(parameters, param_count, param_names)
            desc, col_convs = _resolve_description_and_converters(
                col_meta, self._connection._detect_types
            )
            self._description = desc
            self._col_converters = col_convs

            tf_tag = _encode_text_factory(self._connection._text_factory)
            has_row, row, changes, lastrowid = _stmt_bind_and_step(
                stmt, bound, tf_tag, col_convs
            )
            if has_row:
                self._pending_row = row
                self._has_pending = True
                self._exhausted = False
                self._rowcount = -1
            else:
                self._pending_row = None
                self._has_pending = False
                self._exhausted = True
                if is_dml:
                    self._rowcount = changes
                    self._lastrowid = lastrowid
                else:
                    self._rowcount = -1
                    if col_count == 0:
                        self._lastrowid = lastrowid
        except Exception:
            self._close_stmt()
            raise

        return self

    def executemany(self, sql, seq_of_parameters, /):
        self._check_open()
        if not isinstance(sql, str):
            raise TypeError(
                f"executemany() argument 1 must be str, not {type(sql).__name__}"
            )
        self._close_stmt()

        stmt, is_dml, col_count, _col_meta, param_count, param_names = _stmt_prepare(
            self._connection._handle, sql
        )
        if stmt is None:
            self._description = None
            self._col_converters = []
            self._rowcount = -1
            return self

        self._stmt = stmt
        try:
            if col_count > 0 or not is_dml:
                raise ProgrammingError(
                    "executemany() can only execute DML statements."
                )

            self._description = None
            self._col_converters = []

            if (
                self._connection._autocommit == LEGACY_TRANSACTION_CONTROL
                and self._connection._isolation_level is not None
                and not self._connection.in_transaction
            ):
                begin_sql = (
                    f"BEGIN {self._connection._isolation_level}".rstrip()
                )
                _conn_exec_simple(self._connection._handle, begin_sql)

            tf_tag = _encode_text_factory(self._connection._text_factory)
            total_changes = 0
            for params in seq_of_parameters:
                bound = self._resolve_params(params, param_count, param_names)
                _has_row, _row, changes, _lastrowid = _stmt_bind_and_step(
                    stmt, bound, tf_tag, []
                )
                total_changes += changes
            self._rowcount = total_changes
            self._exhausted = True
        finally:
            self._close_stmt()

        return self

    def executescript(self, sql_script, /):
        self._check_open()
        if not isinstance(sql_script, str):
            raise TypeError(
                f"executescript() argument 1 must be str, not {type(sql_script).__name__}"
            )
        self._close_stmt()
        self._description = None
        self._col_converters = []
        if (
            self._connection._autocommit == LEGACY_TRANSACTION_CONTROL
            and self._connection.in_transaction
        ):
            _conn_exec_simple(self._connection._handle, "COMMIT")
        _conn_executescript(self._connection._handle, sql_script)
        return self

    def fetchone(self):
        self._check_open()
        if self._has_pending:
            row = self._pending_row
            self._pending_row = None
            self._has_pending = False
            return self._format_row(row)
        if self._exhausted or self._stmt is None:
            return None
        tf_tag = _encode_text_factory(self._connection._text_factory)
        has_row, row = _stmt_step(self._stmt, tf_tag, self._col_converters)
        if not has_row:
            self._exhausted = True
            return None
        return self._format_row(row)

    def fetchmany(self, size=None):
        self._check_open()
        if size is None:
            size = self._arraysize
        if not isinstance(size, int) or isinstance(size, bool):
            raise TypeError(f"size must be int, not {type(size).__name__}")
        rows = []
        if size <= 0:
            return rows
        for _ in range(size):
            row = self.fetchone()
            if row is None:
                break
            rows.append(row)
        return rows

    def fetchall(self):
        self._check_open()
        rows = []
        while True:
            row = self.fetchone()
            if row is None:
                break
            rows.append(row)
        return rows

    def close(self):
        if not self._closed:
            self._close_stmt()
            self._closed = True

    def setinputsizes(self, sizes, /):
        self._check_open()

    def setoutputsize(self, size, column=None, /):
        self._check_open()

    def __iter__(self):
        self._check_open()
        return self

    def __next__(self):
        row = self.fetchone()
        if row is None:
            raise StopIteration
        return row


# ── Connection ───────────────────────────────────────────────────────────────


class Connection:
    """SQLite database connection."""

    def __init__(
        self,
        database,
        timeout=5.0,
        detect_types=0,
        isolation_level="",
        check_same_thread=True,
        factory=None,
        cached_statements=128,
        uri=False,
        *,
        autocommit=LEGACY_TRANSACTION_CONTROL,
    ):
        # Resolve database path (str, bytes, or os.PathLike)
        path = os.fspath(database)
        if isinstance(path, bytes):
            path = path.decode("utf-8")
        elif not isinstance(path, str):
            raise TypeError(
                f"database must be str, bytes, or os.PathLike, not {type(database).__name__}"
            )

        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)):
            raise TypeError(
                f"timeout must be a real number, not {type(timeout).__name__}"
            )
        if not isinstance(detect_types, int) or isinstance(detect_types, bool):
            raise TypeError(
                f"detect_types must be int, not {type(detect_types).__name__}"
            )
        iso = _validate_isolation_level(isolation_level)
        ac = _validate_autocommit(autocommit)

        timeout_ms = int(float(timeout) * 1000.0)
        self._handle = _conn_open(path, timeout_ms, bool(uri))
        self._closed = False
        self._detect_types = detect_types
        self._isolation_level = iso
        self._autocommit = ac
        self._check_same_thread = bool(check_same_thread)
        self._cached_statements = cached_statements
        self._row_factory = None
        self._text_factory = str

        if ac is False:
            _conn_exec_simple(self._handle, "BEGIN")

    def _check_open(self):
        if self._closed or self._handle is None:
            raise ProgrammingError("Cannot operate on a closed database.")

    # DB-API 2.0 exception attributes on Connection instances
    @property
    def Warning(self):
        return Warning

    @property
    def Error(self):
        return Error

    @property
    def InterfaceError(self):
        return InterfaceError

    @property
    def DatabaseError(self):
        return DatabaseError

    @property
    def DataError(self):
        return DataError

    @property
    def OperationalError(self):
        return OperationalError

    @property
    def IntegrityError(self):
        return IntegrityError

    @property
    def InternalError(self):
        return InternalError

    @property
    def ProgrammingError(self):
        return ProgrammingError

    @property
    def NotSupportedError(self):
        return NotSupportedError

    @property
    def row_factory(self):
        return self._row_factory

    @row_factory.setter
    def row_factory(self, value):
        self._row_factory = value

    @property
    def text_factory(self):
        return self._text_factory

    @text_factory.setter
    def text_factory(self, value):
        self._text_factory = value

    @property
    def isolation_level(self):
        return self._isolation_level

    @isolation_level.setter
    def isolation_level(self, value):
        iso = _validate_isolation_level(value)
        if (
            iso is None
            and not self._closed
            and self._autocommit == LEGACY_TRANSACTION_CONTROL
            and self.in_transaction
        ):
            self.commit()
        self._isolation_level = iso

    @property
    def autocommit(self):
        return self._autocommit

    @autocommit.setter
    def autocommit(self, value):
        self._check_open()
        ac = _validate_autocommit(value)
        if ac is True:
            if self.in_transaction:
                _conn_exec_simple(self._handle, "COMMIT")
        elif ac is False:
            if not self.in_transaction:
                _conn_exec_simple(self._handle, "BEGIN")
        self._autocommit = ac

    @property
    def in_transaction(self):
        if self._closed or self._handle is None:
            return False
        return _conn_in_transaction(self._handle)

    @property
    def total_changes(self):
        self._check_open()
        return _conn_total_changes(self._handle)

    def cursor(self, factory=Cursor):
        self._check_open()
        cur = factory(self)
        if not isinstance(cur, Cursor):
            raise TypeError(
                "factory must return a Cursor instance"
            )
        return cur

    def execute(self, sql, parameters=(), /):
        cur = self.cursor()
        return cur.execute(sql, parameters)

    def executemany(self, sql, parameters, /):
        cur = self.cursor()
        return cur.executemany(sql, parameters)

    def executescript(self, sql_script, /):
        cur = self.cursor()
        return cur.executescript(sql_script)

    def commit(self):
        self._check_open()
        if self._autocommit is True:
            return
        if self.in_transaction:
            _conn_exec_simple(self._handle, "COMMIT")
        if self._autocommit is False:
            _conn_exec_simple(self._handle, "BEGIN")

    def rollback(self):
        self._check_open()
        if self._autocommit is True:
            return
        if self.in_transaction:
            _conn_exec_simple(self._handle, "ROLLBACK")
        if self._autocommit is False:
            _conn_exec_simple(self._handle, "BEGIN")

    def close(self):
        if not self._closed and self._handle is not None:
            if self._autocommit is False and _conn_in_transaction(self._handle):
                try:
                    _conn_exec_simple(self._handle, "ROLLBACK")
                except Exception:
                    pass
            _conn_close(self._handle)
            self._handle = None
            self._closed = True

    def interrupt(self):
        self._check_open()
        _conn_interrupt(self._handle)

    def create_function(self, name, narg, func, *, deterministic=False):
        self._check_open()
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        if not isinstance(narg, int) or isinstance(narg, bool):
            raise TypeError(f"narg must be int, not {type(narg).__name__}")
        if func is not None and not callable(func):
            raise TypeError("func must be callable or None")
        _conn_create_function(self._handle, name, narg, func, bool(deterministic))

    def create_aggregate(self, name, narg, aggregate_class):
        self._check_open()
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        if not isinstance(narg, int) or isinstance(narg, bool):
            raise TypeError(f"narg must be int, not {type(narg).__name__}")
        if aggregate_class is not None and not callable(aggregate_class):
            raise TypeError("aggregate_class must be callable or None")
        _conn_create_aggregate(self._handle, name, narg, aggregate_class)

    def create_collation(self, name, callable_, /):
        self._check_open()
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        if callable_ is not None and not callable(callable_):
            raise TypeError("parameter must be callable")
        _conn_create_collation(self._handle, name, callable_)

    def set_authorizer(self, authorizer_callback):
        self._check_open()
        if authorizer_callback is not None and not callable(authorizer_callback):
            raise TypeError("authorizer_callback must be callable or None")
        _conn_set_authorizer(self._handle, authorizer_callback)

    def set_progress_handler(self, progress_handler, n):
        self._check_open()
        if progress_handler is not None and not callable(progress_handler):
            raise TypeError("progress_handler must be callable or None")
        if not isinstance(n, int) or isinstance(n, bool):
            raise TypeError(f"n must be int, not {type(n).__name__}")
        _conn_set_progress_handler(self._handle, progress_handler, n)

    def set_trace_callback(self, trace_callback):
        self._check_open()
        if trace_callback is not None and not callable(trace_callback):
            raise TypeError("trace_callback must be callable or None")
        _conn_set_trace_callback(self._handle, trace_callback)

    def iterdump(self):
        self._check_open()
        return _iterdump(self)

    def backup(self, target, *, pages=-1, progress=None, name="main", sleep=0.25):
        self._check_open()
        if not isinstance(target, Connection):
            raise TypeError(
                f"backup() argument 'target' must be sqlite3.Connection, not {type(target).__name__}"
            )
        target._check_open()
        if not isinstance(pages, int) or isinstance(pages, bool):
            raise TypeError(f"pages must be int, not {type(pages).__name__}")
        if progress is not None and not callable(progress):
            raise TypeError("progress must be callable or None")
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        if isinstance(sleep, bool) or not isinstance(sleep, (int, float)):
            raise TypeError(f"sleep must be a real number, not {type(sleep).__name__}")
        _conn_backup(
            self._handle, target._handle, pages, progress, name, float(sleep)
        )

    def blobopen(self, table, column, row, /, *, readonly=False, name="main"):
        self._check_open()
        if not isinstance(table, str):
            raise TypeError(f"table must be str, not {type(table).__name__}")
        if not isinstance(column, str):
            raise TypeError(f"column must be str, not {type(column).__name__}")
        if not isinstance(row, int) or isinstance(row, bool):
            raise TypeError(f"row must be int, not {type(row).__name__}")
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        handle = _blob_open(self._handle, name, table, column, row, bool(readonly))
        return Blob(handle)

    def getlimit(self, category, /):
        self._check_open()
        if not isinstance(category, int) or isinstance(category, bool):
            raise TypeError(f"category must be int, not {type(category).__name__}")
        return _conn_getlimit(self._handle, category)

    def setlimit(self, category, limit, /):
        self._check_open()
        if not isinstance(category, int) or isinstance(category, bool):
            raise TypeError(f"category must be int, not {type(category).__name__}")
        if not isinstance(limit, int) or isinstance(limit, bool):
            raise TypeError(f"limit must be int, not {type(limit).__name__}")
        return _conn_setlimit(self._handle, category, limit)

    def getconfig(self, op, /):
        self._check_open()
        if not isinstance(op, int) or isinstance(op, bool):
            raise TypeError(f"op must be int, not {type(op).__name__}")
        return _conn_getconfig(self._handle, op)

    def setconfig(self, op, enable=True, /):
        self._check_open()
        if not isinstance(op, int) or isinstance(op, bool):
            raise TypeError(f"op must be int, not {type(op).__name__}")
        return _conn_setconfig(self._handle, op, bool(enable))

    def serialize(self, *, name="main"):
        self._check_open()
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        return _conn_serialize(self._handle, name)

    def deserialize(self, data, /, *, name="main"):
        self._check_open()
        if not isinstance(name, str):
            raise TypeError(f"name must be str, not {type(name).__name__}")
        _conn_deserialize(self._handle, data, name)

    def __enter__(self):
        self._check_open()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is None:
            self.commit()
        else:
            self.rollback()
        return False


# ── connect() ────────────────────────────────────────────────────────────────


def connect(
    database,
    timeout=5.0,
    detect_types=0,
    isolation_level="",
    check_same_thread=True,
    factory=Connection,
    cached_statements=128,
    uri=False,
    *,
    autocommit=LEGACY_TRANSACTION_CONTROL,
):
    """Open a connection to an SQLite database."""
    return factory(
        database,
        timeout=timeout,
        detect_types=detect_types,
        isolation_level=isolation_level,
        check_same_thread=check_same_thread,
        cached_statements=cached_statements,
        uri=uri,
        autocommit=autocommit,
    )
