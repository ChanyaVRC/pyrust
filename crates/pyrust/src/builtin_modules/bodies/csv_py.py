"""Pure-Python implementation of the standard library ``csv`` module.

Targets CPython 3.12 behaviour for the public surface that pyrust exercises:
the ``reader`` / ``writer`` factories, the ``DictReader`` / ``DictWriter``
helpers, the ``Dialect`` family (``excel`` / ``excel_tab`` / ``unix_dialect``),
the dialect registry, and the ``QUOTE_*`` constants.

Reference: https://docs.python.org/3/library/csv.html
"""

# --- quoting constants ------------------------------------------------------

QUOTE_MINIMAL = 0
QUOTE_ALL = 1
QUOTE_NONNUMERIC = 2
QUOTE_NONE = 3


class Error(Exception):
    __module__ = "_csv"


# --- dialects ---------------------------------------------------------------


class Dialect:
    """Describe a CSV dialect.

    Subclasses (or instances built from ``fmtparams``) carry the attributes
    consulted by the reader and writer.
    """

    _name = ""
    delimiter = ","
    quotechar = '"'
    escapechar = None
    doublequote = True
    skipinitialspace = False
    lineterminator = "\r\n"
    quoting = QUOTE_MINIMAL
    strict = False

    def __init__(self):
        if self.delimiter is None or len(self.delimiter) != 1:
            raise TypeError('"delimiter" must be a 1-character string')
        if self.quotechar is None and self.quoting != QUOTE_NONE:
            raise TypeError("quotechar must be set if quoting enabled")


class excel(Dialect):
    delimiter = ","
    quotechar = '"'
    doublequote = True
    skipinitialspace = False
    lineterminator = "\r\n"
    quoting = QUOTE_MINIMAL


class excel_tab(excel):
    delimiter = "\t"


class unix_dialect(Dialect):
    delimiter = ","
    quotechar = '"'
    doublequote = True
    skipinitialspace = False
    lineterminator = "\n"
    quoting = QUOTE_ALL


_dialects = {
    "excel": excel,
    "excel-tab": excel_tab,
    "unix": unix_dialect,
}


def register_dialect(name, dialect=None, **fmtparams):
    if dialect is None:
        d = _make_dialect("excel", fmtparams)
    elif isinstance(dialect, type) and issubclass(dialect, Dialect):
        d = _make_dialect(dialect, fmtparams)
    else:
        d = _make_dialect(dialect, fmtparams)
    _dialects[name] = d


def unregister_dialect(name):
    try:
        del _dialects[name]
    except KeyError:
        raise Error("unknown dialect")


def get_dialect(name):
    try:
        return _dialects[name]
    except KeyError:
        raise Error("unknown dialect")


def list_dialects():
    return list(_dialects.keys())


_FMT_ATTRS = (
    "delimiter",
    "quotechar",
    "escapechar",
    "doublequote",
    "skipinitialspace",
    "lineterminator",
    "quoting",
    "strict",
)


def _resolve_base(dialect):
    """Return a base Dialect instance for ``dialect`` (name, class, or instance)."""
    if dialect is None:
        return excel()
    if isinstance(dialect, str):
        base = get_dialect(dialect)
        return base() if isinstance(base, type) else base
    if isinstance(dialect, type) and issubclass(dialect, Dialect):
        return dialect()
    if isinstance(dialect, Dialect):
        return dialect
    raise TypeError("dialect must be a string, Dialect subclass, or instance")


def _make_dialect(dialect, fmtparams):
    base = _resolve_base(dialect)
    d = Dialect.__new__(Dialect)
    for attr in _FMT_ATTRS:
        setattr(d, attr, getattr(base, attr))
    for key, value in fmtparams.items():
        if key not in _FMT_ATTRS:
            raise TypeError("unexpected keyword argument %r" % (key,))
        setattr(d, key, value)
    Dialect.__init__(d)
    return d


# --- reader -----------------------------------------------------------------


# Parser states, named after CPython's ``_csv.c`` ``ParserState`` enum.
(
    _START_RECORD,
    _START_FIELD,
    _ESCAPED_CHAR,
    _IN_FIELD,
    _IN_QUOTED_FIELD,
    _ESCAPE_IN_QUOTED_FIELD,
    _QUOTE_IN_QUOTED_FIELD,
    _EAT_CRNL,
    _AFTER_ESCAPED_CRNL,
) = range(9)

# End-of-line marker fed to the state machine after the last character of a
# physical line.  The empty string never equals a one-character dialect
# attribute (nor ``None``), so it cannot be mistaken for real input.
_EOL = ""


class _reader:
    """CSV parser mirroring the reader in CPython's ``_csv.c``.

    The state machine is carried across the physical lines pulled from the
    input iterable, so a quoted field or an escaped newline can span several
    lines.  ``line_num`` counts the physical lines consumed.
    """

    def __init__(self, iterable, dialect):
        self._input = iter(iterable)
        self.dialect = dialect
        self.line_num = 0

    def __iter__(self):
        return self

    def __next__(self):
        d = self.dialect
        delimiter = d.delimiter
        quotechar = d.quotechar
        escapechar = d.escapechar
        doublequote = d.doublequote
        skipinitialspace = d.skipinitialspace
        quoting = d.quoting
        strict = d.strict
        quote_on = quoting != QUOTE_NONE
        nonnumeric = quoting == QUOTE_NONNUMERIC

        fields = []
        field = []
        numeric = False
        state = _START_RECORD

        while True:
            try:
                line = next(self._input)
            except StopIteration:
                # End of input inside a record: CPython reports it under
                # ``strict`` and otherwise yields the partial record.
                if field or state == _IN_QUOTED_FIELD:
                    if strict:
                        raise Error("unexpected end of data")
                    fields.append(self._finish(field, numeric))
                    return fields
                raise
            if not isinstance(line, str):
                raise Error(
                    "iterator should return strings, not %s "
                    "(the file should be opened in text mode)"
                    % type(line).__name__
                )
            self.line_num += 1

            n = len(line)
            pos = 0
            while pos <= n:
                c = line[pos] if pos < n else _EOL
                pos += 1

                if state == _START_RECORD:
                    if c == _EOL:
                        continue  # empty line -> []
                    if c == "\n" or c == "\r":
                        state = _EAT_CRNL
                        continue
                    state = _START_FIELD
                    # fall through to START_FIELD handling

                if state == _START_FIELD:
                    if c == "\n" or c == "\r" or c == _EOL:
                        fields.append(self._finish(field, numeric))
                        field = []
                        numeric = False
                        state = _START_RECORD if c == _EOL else _EAT_CRNL
                    elif quote_on and c == quotechar:
                        state = _IN_QUOTED_FIELD
                    elif c == escapechar:
                        state = _ESCAPED_CHAR
                    elif c == " " and skipinitialspace:
                        pass
                    elif c == delimiter:
                        fields.append(self._finish(field, numeric))
                        field = []
                        numeric = False
                    else:
                        if nonnumeric:
                            numeric = True
                        field.append(c)
                        state = _IN_FIELD

                elif state == _ESCAPED_CHAR:
                    if c == "\n" or c == "\r":
                        field.append(c)
                        state = _AFTER_ESCAPED_CRNL
                    else:
                        field.append("\n" if c == _EOL else c)
                        state = _IN_FIELD

                elif state == _IN_FIELD or (
                    state == _AFTER_ESCAPED_CRNL and c != _EOL
                ):
                    if c == "\n" or c == "\r" or c == _EOL:
                        fields.append(self._finish(field, numeric))
                        field = []
                        numeric = False
                        state = _START_RECORD if c == _EOL else _EAT_CRNL
                    elif c == escapechar:
                        state = _ESCAPED_CHAR
                    elif c == delimiter:
                        fields.append(self._finish(field, numeric))
                        field = []
                        numeric = False
                        state = _START_FIELD
                    else:
                        field.append(c)

                elif state == _AFTER_ESCAPED_CRNL:
                    pass  # c is EOL: keep waiting for the next line

                elif state == _IN_QUOTED_FIELD:
                    if c == _EOL:
                        pass
                    elif c == escapechar:
                        state = _ESCAPE_IN_QUOTED_FIELD
                    elif quote_on and c == quotechar:
                        state = _QUOTE_IN_QUOTED_FIELD if doublequote else _IN_FIELD
                    else:
                        field.append(c)

                elif state == _ESCAPE_IN_QUOTED_FIELD:
                    field.append("\n" if c == _EOL else c)
                    state = _IN_QUOTED_FIELD

                elif state == _QUOTE_IN_QUOTED_FIELD:
                    if quote_on and c == quotechar:
                        field.append(c)
                        state = _IN_QUOTED_FIELD
                    elif c == delimiter:
                        fields.append(self._finish(field, numeric))
                        field = []
                        numeric = False
                        state = _START_FIELD
                    elif c == "\n" or c == "\r" or c == _EOL:
                        fields.append(self._finish(field, numeric))
                        field = []
                        numeric = False
                        state = _START_RECORD if c == _EOL else _EAT_CRNL
                    elif not strict:
                        field.append(c)
                        state = _IN_FIELD
                    else:
                        raise Error(
                            "'%s' expected after '%s'" % (delimiter, quotechar)
                        )

                else:  # _EAT_CRNL
                    if c == "\n" or c == "\r":
                        pass
                    elif c == _EOL:
                        state = _START_RECORD
                    else:
                        raise Error(
                            "new-line character seen in unquoted field - do "
                            "you need to open the file with newline=''?"
                        )

            if state == _START_RECORD:
                return fields

    @staticmethod
    def _finish(field, numeric):
        value = "".join(field)
        if numeric:
            return float(value)
        return value


def reader(csvfile, dialect="excel", **fmtparams):
    return _reader(csvfile, _make_dialect(dialect, fmtparams))


# --- writer -----------------------------------------------------------------


class _writer:
    def __init__(self, fileobj, dialect):
        self._file = fileobj
        self.dialect = dialect

    def writerow(self, row):
        d = self.dialect
        cols = [self._format_field(field) for field in row]
        # CPython special case (csv_writerow): a record of a single field whose
        # rendered form is empty must be quoted, otherwise the line would read
        # back as an empty record ([]).  QUOTE_NONE cannot quote, so it raises.
        if len(cols) == 1 and cols[0] == "":
            if d.quoting == QUOTE_NONE or d.quotechar is None:
                raise Error("single empty field record must be quoted")
            cols[0] = d.quotechar + d.quotechar
        out = d.delimiter.join(cols)
        out += d.lineterminator
        return self._file.write(out)

    def writerows(self, rows):
        for row in rows:
            self.writerow(row)

    def _format_field(self, field):
        d = self.dialect
        quoting = d.quoting
        delimiter = d.delimiter
        quotechar = d.quotechar
        escapechar = d.escapechar

        is_numeric = isinstance(field, (int, float)) and not isinstance(field, bool)
        if field is None:
            text = ""
        else:
            text = field if isinstance(field, str) else str(field)

        if quoting == QUOTE_NONE:
            if escapechar is None:
                # Mirror CPython: still escape special chars if possible,
                # otherwise the join would corrupt the row.  CPython raises
                # when an escapechar is required but absent.
                specials = delimiter + (quotechar or "") + "\r\n"
                if any(c in specials for c in text):
                    raise Error("need to escape, but no escapechar set")
                return text
            escaped = []
            specials = delimiter + (quotechar or "") + escapechar + "\r\n"
            for c in text:
                if c in specials:
                    escaped.append(escapechar)
                escaped.append(c)
            return "".join(escaped)

        # Mirror CPython's `join_append_data` (Modules/_csv.c): walk the field a
        # character at a time, deciding per-character whether to force quoting
        # and whether to escape.  QUOTE_ALL / QUOTE_NONNUMERIC seed `quoted`;
        # QUOTE_MINIMAL starts unquoted and only the delimiter / CR / LF (and a
        # quotechar handled by doubling) force it.
        doublequote = d.doublequote
        if quoting == QUOTE_ALL:
            quoted = True
        elif quoting == QUOTE_NONNUMERIC:
            quoted = not is_numeric
        else:  # QUOTE_MINIMAL
            quoted = False

        out = []
        for c in text:
            if c == delimiter or c == "\r" or c == "\n":
                quoted = True
            elif quotechar is not None and c == quotechar:
                if doublequote:
                    quoted = True
                    out.append(quotechar)
                else:
                    if escapechar is None:
                        raise Error("need to escape, but no escapechar set")
                    out.append(escapechar)
            elif escapechar is not None and c == escapechar:
                out.append(escapechar)
            out.append(c)

        text = "".join(out)
        if quoted and quotechar is not None:
            return quotechar + text + quotechar
        return text


def writer(csvfile, dialect="excel", **fmtparams):
    return _writer(csvfile, _make_dialect(dialect, fmtparams))


# --- dict helpers -----------------------------------------------------------


class DictReader:
    def __init__(
        self,
        f,
        fieldnames=None,
        restkey=None,
        restval=None,
        dialect="excel",
        *args,
        **kwds,
    ):
        self._fieldnames = fieldnames
        self.restkey = restkey
        self.restval = restval
        self.reader = reader(f, dialect, *args, **kwds)
        self.dialect = dialect
        self.line_num = 0

    def __iter__(self):
        return self

    @property
    def fieldnames(self):
        if self._fieldnames is None:
            try:
                self._fieldnames = next(self.reader)
            except StopIteration:
                pass
        self.line_num = self.reader.line_num
        return self._fieldnames

    @fieldnames.setter
    def fieldnames(self, value):
        self._fieldnames = value

    def __next__(self):
        if self.line_num == 0:
            # Used only for its side effect of reading the header row.
            self.fieldnames
        row = next(self.reader)
        self.line_num = self.reader.line_num

        # Skip blank rows, as CPython does.
        while row == []:
            row = next(self.reader)
        self.line_num = self.reader.line_num

        d = dict(zip(self.fieldnames, row))
        lf = len(self.fieldnames)
        lr = len(row)
        if lf < lr:
            d[self.restkey] = row[lf:]
        elif lf > lr:
            for key in self.fieldnames[lr:]:
                d[key] = self.restval
        return d


class DictWriter:
    def __init__(
        self,
        f,
        fieldnames,
        restval="",
        extrasaction="raise",
        dialect="excel",
        *args,
        **kwds,
    ):
        self.fieldnames = fieldnames
        self.restval = restval
        if extrasaction.lower() not in ("raise", "ignore"):
            raise ValueError("extrasaction (%s) must be 'raise' or 'ignore'" % extrasaction)
        self.extrasaction = extrasaction
        self.writer = writer(f, dialect, *args, **kwds)

    def writeheader(self):
        header = dict(zip(self.fieldnames, self.fieldnames))
        return self.writerow(header)

    def _dict_to_list(self, rowdict):
        if self.extrasaction == "raise":
            wrong_fields = rowdict.keys() - self.fieldnames
            if wrong_fields:
                raise ValueError(
                    "dict contains fields not in fieldnames: "
                    + ", ".join([repr(x) for x in wrong_fields])
                )
        return (rowdict.get(key, self.restval) for key in self.fieldnames)

    def writerow(self, rowdict):
        return self.writer.writerow(self._dict_to_list(rowdict))

    def writerows(self, rowdicts):
        return self.writer.writerows(self._dict_to_list(rowdict) for rowdict in rowdicts)
