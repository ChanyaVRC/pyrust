"""Extract, format and print information about Python stack traces."""

import collections
import collections.abc
import itertools
import os
import sys
import textwrap

__all__ = [
    'extract_stack',
    'extract_tb',
    'format_exception',
    'format_exception_only',
    'format_list',
    'format_stack',
    'format_tb',
    'print_exc',
    'format_exc',
    'print_exception',
    'print_last',
    'print_stack',
    'print_tb',
    'clear_frames',
    'FrameSummary',
    'StackSummary',
    'TracebackException',
    'walk_stack',
    'walk_tb',
]

# Lightweight source-line cache (pyrust does not ship a standalone `linecache`
# module yet).  Entries map filename -> (size, mtime, lines).
_line_cache = {}


def _checkcache(filename=None):
    if filename is None:
        filenames = list(_line_cache.keys())
    elif filename in _line_cache:
        filenames = [filename]
    else:
        return
    for name in filenames:
        entry = _line_cache.get(name)
        if entry is None:
            continue
        size, mtime, _ = entry
        if mtime is None:
            continue
        try:
            st = os.stat(name)
        except OSError:
            _line_cache.pop(name, None)
            continue
        if size != st.st_size or mtime != st.st_mtime:
            _line_cache.pop(name, None)


def _getlines(filename):
    if not filename:
        return []
    entry = _line_cache.get(filename)
    if entry is not None:
        return entry[2]
    if filename.startswith('<') and filename.endswith('>'):
        return []
    try:
        st = os.stat(filename)
        with open(filename, 'r', encoding='utf-8', errors='replace') as fp:
            lines = fp.readlines()
    except OSError:
        return []
    if lines and not lines[-1].endswith('\n'):
        lines[-1] += '\n'
    _line_cache[filename] = (st.st_size, st.st_mtime, lines)
    return lines


def _getline(filename, lineno):
    if lineno is None or lineno < 1:
        return ''
    lines = _getlines(filename)
    if 1 <= lineno <= len(lines):
        return lines[lineno - 1]
    return ''


#
# Formatting and printing lists of traceback lines.
#

def print_list(extracted_list, file=None):
    """Print the list of tuples as returned by extract_tb() or
    extract_stack() as a formatted stack trace to the given file."""
    if file is None:
        file = sys.stderr
    for item in StackSummary.from_list(extracted_list).format():
        print(item, file=file, end="")


def format_list(extracted_list):
    """Format a list of tuples or FrameSummary objects for printing.

    Given a list of tuples or FrameSummary objects as returned by
    extract_tb() or extract_stack(), return a list of strings ready
    for printing.

    Each string in the resulting list corresponds to the item with the
    same index in the argument list.  Each string ends in a newline;
    the strings may contain internal newlines as well, for those items
    whose source text line is not None.
    """
    return StackSummary.from_list(extracted_list).format()


#
# Printing and Extracting Tracebacks.
#

def print_tb(tb, limit=None, file=None):
    """Print up to 'limit' stack trace entries from the traceback 'tb'.

    If 'limit' is omitted or None, all entries are printed.  If 'file'
    is omitted or None, the output goes to sys.stderr; otherwise
    'file' should be an open file or file-like object with a write()
    method.
    """
    print_list(extract_tb(tb, limit=limit), file=file)


def format_tb(tb, limit=None):
    """A shorthand for 'format_list(extract_tb(tb, limit))'."""
    return extract_tb(tb, limit=limit).format()


def extract_tb(tb, limit=None):
    """Return a StackSummary object representing a list of
    pre-processed entries from traceback.

    This is useful for alternate formatting of stack traces.  If
    'limit' is omitted or None, all entries are extracted.  A
    pre-processed stack trace entry is a FrameSummary object
    containing attributes filename, lineno, name, and line
    representing the information that is usually printed for a stack
    trace.  The line is a string with leading and trailing
    whitespace stripped; if the source is not available it is None.
    """
    return StackSummary._extract_from_extended_frame_gen(
        _walk_tb_with_full_positions(tb), limit=limit
    )


#
# Exception formatting and output.
#

_cause_message = (
    "\nThe above exception was the direct cause "
    "of the following exception:\n\n"
)

_context_message = (
    "\nDuring handling of the above exception, "
    "another exception occurred:\n\n"
)


class _Sentinel:
    def __repr__(self):
        return "<implicit>"


_sentinel = _Sentinel()


def _parse_value_tb(exc, value, tb):
    if (value is _sentinel) != (tb is _sentinel):
        raise ValueError("Both or neither of value and tb must be given")
    if value is tb is _sentinel:
        if exc is not None:
            if isinstance(exc, BaseException):
                return exc, exc.__traceback__
            raise TypeError(
                f'Exception expected for value, {type(exc).__name__} found'
            )
        else:
            return None, None
    return value, tb


def print_exception(exc, /, value=_sentinel, tb=_sentinel, limit=None,
                    file=None, chain=True):
    """Print exception up to 'limit' stack trace entries from 'tb' to 'file'."""
    value, tb = _parse_value_tb(exc, value, tb)
    te = TracebackException(type(value), value, tb, limit=limit, compact=True)
    te.print(file=file, chain=chain)


def format_exception(exc, /, value=_sentinel, tb=_sentinel, limit=None,
                     chain=True):
    """Format a stack trace and the exception information."""
    value, tb = _parse_value_tb(exc, value, tb)
    te = TracebackException(type(value), value, tb, limit=limit, compact=True)
    return list(te.format(chain=chain))


def format_exception_only(exc, /, value=_sentinel, *, show_group=False):
    """Format the exception part of a traceback.

    The return value is a list of strings, each ending in a newline.
    """
    if value is _sentinel:
        value = exc
    te = TracebackException(type(value), value, None, compact=True)
    return list(te.format_exception_only(show_group=show_group))


# -- not official API but folk probably use these two functions.

def _format_final_exc_line(etype, value):
    valuestr = _safe_string(value, 'exception')
    if value is None or not valuestr:
        line = "%s\n" % etype
    else:
        line = "%s: %s\n" % (etype, valuestr)
    return line


def _safe_string(value, what, func=str):
    try:
        return func(value)
    except Exception:
        return f'<{what} {func.__name__}() failed>'


# --

def print_exc(limit=None, file=None, chain=True):
    """Shorthand for 'print_exception(sys.exception(), limit=limit, file=file, chain=chain)'."""
    print_exception(sys.exception(), limit=limit, file=file, chain=chain)


def format_exc(limit=None, chain=True):
    """Like print_exc() but return a string."""
    return "".join(format_exception(sys.exception(), limit=limit, chain=chain))


def print_last(limit=None, file=None, chain=True):
    """This is a shorthand for 'print_exception(sys.last_exc, limit=limit, file=file, chain=chain)'."""
    if not hasattr(sys, "last_exc") and not hasattr(sys, "last_type"):
        raise ValueError("no last exception")

    if hasattr(sys, "last_exc"):
        print_exception(sys.last_exc, limit=limit, file=file, chain=chain)
    else:
        print_exception(
            sys.last_type,
            sys.last_value,
            sys.last_traceback,
            limit=limit,
            file=file,
            chain=chain,
        )


#
# Printing and Extracting Stacks.
#

def print_stack(f=None, limit=None, file=None):
    """Print a stack trace from its invocation point."""
    if f is None:
        f = sys._getframe().f_back
    print_list(extract_stack(f, limit=limit), file=file)


def format_stack(f=None, limit=None):
    """Shorthand for 'format_list(extract_stack(f, limit))'."""
    if f is None:
        f = sys._getframe().f_back
    return format_list(extract_stack(f, limit=limit))


def extract_stack(f=None, limit=None):
    """Extract the raw traceback from the current stack frame."""
    if f is None:
        f = sys._getframe().f_back
    stack = StackSummary.extract(walk_stack(f), limit=limit)
    stack.reverse()
    return stack


def clear_frames(tb):
    """Clear all references to local variables in the frames of a traceback."""
    active_ids = set()
    cur = sys._getframe()
    while cur is not None:
        active_ids.add(id(cur))
        cur = cur.f_back
    while tb is not None:
        frame = tb.tb_frame
        clear = getattr(frame, 'clear', None)
        if clear is not None:
            try:
                clear()
            except RuntimeError:
                pass
        elif id(frame) not in active_ids:
            try:
                f_locals = frame.f_locals
                f_globals = frame.f_globals
                if f_locals is not f_globals and hasattr(f_locals, 'clear'):
                    f_locals.clear()
            except (AttributeError, RuntimeError):
                pass
        tb = tb.tb_next


class FrameSummary:
    """Information about a single frame from a traceback."""

    __slots__ = (
        'filename',
        'lineno',
        'end_lineno',
        'colno',
        'end_colno',
        'name',
        '_line',
        'locals',
    )

    def __init__(self, filename, lineno, name, *, lookup_line=True,
                 locals=None, line=None,
                 end_lineno=None, colno=None, end_colno=None):
        self.filename = filename
        self.lineno = lineno
        self.name = name
        self._line = line
        if lookup_line:
            self.line
        self.locals = (
            {k: _safe_string(v, 'local', func=repr) for k, v in locals.items()}
            if locals
            else None
        )
        self.end_lineno = end_lineno
        self.colno = colno
        self.end_colno = end_colno

    def __eq__(self, other):
        if isinstance(other, FrameSummary):
            return (
                self.filename == other.filename
                and self.lineno == other.lineno
                and self.name == other.name
                and self.locals == other.locals
            )
        if isinstance(other, tuple):
            return (self.filename, self.lineno, self.name, self.line) == other
        return NotImplemented

    def __getitem__(self, pos):
        return (self.filename, self.lineno, self.name, self.line)[pos]

    def __iter__(self):
        return iter([self.filename, self.lineno, self.name, self.line])

    def __repr__(self):
        return "<FrameSummary file {filename}, line {lineno} in {name}>".format(
            filename=self.filename, lineno=self.lineno, name=self.name
        )

    def __len__(self):
        return 4

    @property
    def _original_line(self):
        self.line
        return self._line

    @property
    def line(self):
        if self._line is None:
            if self.lineno is None:
                return None
            self._line = _getline(self.filename, self.lineno)
        return self._line.strip()


def walk_stack(f):
    """Walk a stack yielding the frame and line number for each frame.

    This will follow f.f_back from the given frame. If no frame is given, the
    current stack is used. Usually used with StackSummary.extract.
    """
    if f is None:
        f = sys._getframe()
        for _ in range(4):
            if f is None:
                break
            f = f.f_back
    while f is not None:
        yield f, f.f_lineno
        f = f.f_back


def walk_tb(tb):
    """Walk a traceback yielding the frame and line number for each frame.

    This will follow tb.tb_next (and thus is in the opposite order to
    walk_stack). Usually used with StackSummary.extract.
    """
    while tb is not None:
        yield tb.tb_frame, tb.tb_lineno
        tb = tb.tb_next


def _walk_tb_with_full_positions(tb):
    while tb is not None:
        positions = _get_code_position(tb.tb_frame.f_code, tb.tb_lasti)
        if positions[0] is None:
            yield tb.tb_frame, (tb.tb_lineno,) + positions[1:]
        else:
            yield tb.tb_frame, positions
        tb = tb.tb_next


def _get_code_position(code, instruction_index):
    if instruction_index < 0:
        return (None, None, None, None)
    co_positions = getattr(code, 'co_positions', None)
    if co_positions is None:
        return (None, None, None, None)
    positions_gen = co_positions()
    return next(itertools.islice(positions_gen, instruction_index // 2, None))


_RECURSIVE_CUTOFF = 3


def _byte_offset_to_character_offset(s, offset):
    as_utf8 = s.encode('utf-8')
    return len(as_utf8[:offset].decode('utf-8', errors='replace'))


class StackSummary(list):
    """A list of FrameSummary objects, representing a stack of frames."""

    @classmethod
    def extract(klass, frame_gen, *, limit=None, lookup_lines=True,
                capture_locals=False):
        """Create a StackSummary from a traceback or stack object."""
        def extended_frame_gen():
            for f, lineno in frame_gen:
                yield f, (lineno, None, None, None)

        return klass._extract_from_extended_frame_gen(
            extended_frame_gen(),
            limit=limit,
            lookup_lines=lookup_lines,
            capture_locals=capture_locals,
        )

    @classmethod
    def _extract_from_extended_frame_gen(klass, frame_gen, *, limit=None,
                                         lookup_lines=True,
                                         capture_locals=False):
        if limit is None:
            limit = getattr(sys, 'tracebacklimit', None)
            if limit is not None and limit < 0:
                limit = 0
        if limit is not None:
            if limit >= 0:
                frame_gen = itertools.islice(frame_gen, limit)
            else:
                frame_gen = collections.deque(frame_gen, maxlen=-limit)

        result = klass()
        fnames = set()
        for f, (lineno, end_lineno, colno, end_colno) in frame_gen:
            co = f.f_code
            filename = co.co_filename
            name = co.co_name

            fnames.add(filename)
            if capture_locals:
                f_locals = f.f_locals
            else:
                f_locals = None
            result.append(FrameSummary(
                filename,
                lineno,
                name,
                lookup_line=False,
                locals=f_locals,
                end_lineno=end_lineno,
                colno=colno,
                end_colno=end_colno,
            ))
        for filename in fnames:
            _checkcache(filename)
        if lookup_lines:
            for f in result:
                f.line
        return result

    @classmethod
    def from_list(klass, a_list):
        """Create a StackSummary object from a supplied list of
        FrameSummary objects or old-style list of tuples.
        """
        result = StackSummary()
        for frame in a_list:
            if isinstance(frame, FrameSummary):
                result.append(frame)
            else:
                filename, lineno, name, line = frame
                result.append(FrameSummary(filename, lineno, name, line=line))
        return result

    def format_frame_summary(self, frame_summary):
        """Format the lines for a single FrameSummary."""
        row = []
        row.append('  File "{}", line {}, in {}\n'.format(
            frame_summary.filename, frame_summary.lineno, frame_summary.name
        ))
        if frame_summary.line:
            stripped_line = frame_summary.line.strip()
            row.append('    {}\n'.format(stripped_line))

            line = frame_summary._original_line
            if (
                line is not None
                and frame_summary.colno is not None
                and frame_summary.end_colno is not None
            ):
                orig_line_len = len(line)
                frame_line_len = len(frame_summary.line.lstrip())
                stripped_characters = orig_line_len - frame_line_len
                start_offset = _byte_offset_to_character_offset(
                    line, frame_summary.colno
                )
                end_offset = _byte_offset_to_character_offset(
                    line, frame_summary.end_colno
                )
                if frame_summary.lineno != frame_summary.end_lineno:
                    end_offset = len(line.rstrip())
                if end_offset - start_offset < len(stripped_line):
                    dp_start_offset = start_offset + 1
                    dp_end_offset = end_offset + 1
                    row.append('    ')
                    row.append(' ' * max(0, dp_start_offset - stripped_characters))
                    row.append('^' * max(1, dp_end_offset - dp_start_offset))
                    row.append('\n')

        if frame_summary.locals:
            for name, value in sorted(frame_summary.locals.items()):
                row.append('    {name} = {value}\n'.format(name=name, value=value))

        return ''.join(row)

    def format(self):
        """Format the stack ready for printing."""
        result = []
        last_file = None
        last_line = None
        last_name = None
        count = 0
        for frame_summary in self:
            formatted_frame = self.format_frame_summary(frame_summary)
            if formatted_frame is None:
                continue
            diff = (
                last_file is None
                or last_file != frame_summary.filename
                or last_line is None
                or last_line != frame_summary.lineno
                or last_name is None
                or last_name != frame_summary.name
            )
            if diff:
                if count > _RECURSIVE_CUTOFF:
                    count -= _RECURSIVE_CUTOFF
                    plural = "s" if count > 1 else ""
                    result.append(
                        f'  [Previous line repeated {count} more time{plural}]\n'
                    )
                last_file = frame_summary.filename
                last_line = frame_summary.lineno
                last_name = frame_summary.name
                count = 0
            count += 1
            if count > _RECURSIVE_CUTOFF:
                continue
            result.append(formatted_frame)

        if count > _RECURSIVE_CUTOFF:
            count -= _RECURSIVE_CUTOFF
            plural = "s" if count > 1 else ""
            result.append(
                f'  [Previous line repeated {count} more time{plural}]\n'
            )
        return result


class _ExceptionPrintContext:
    def __init__(self):
        self.seen = set()
        self.exception_group_depth = 0
        self.need_close = False

    def indent(self):
        return ' ' * (2 * self.exception_group_depth)

    def emit(self, text_gen, margin_char=None):
        if margin_char is None:
            margin_char = '|'
        indent_str = self.indent()
        if self.exception_group_depth:
            indent_str += margin_char + ' '

        if isinstance(text_gen, str):
            yield textwrap.indent(text_gen, indent_str, lambda line: True)
        else:
            for text in text_gen:
                yield textwrap.indent(text, indent_str, lambda line: True)


class TracebackException:
    """An exception ready for rendering."""

    def __init__(self, exc_type, exc_value, exc_traceback, *, limit=None,
                 lookup_lines=True, capture_locals=False, compact=False,
                 max_group_width=15, max_group_depth=10, _seen=None):
        is_recursive_call = _seen is not None
        if _seen is None:
            _seen = set()
        _seen.add(id(exc_value))

        self.max_group_width = max_group_width
        self.max_group_depth = max_group_depth

        self.stack = StackSummary._extract_from_extended_frame_gen(
            _walk_tb_with_full_positions(exc_traceback),
            limit=limit,
            lookup_lines=lookup_lines,
            capture_locals=capture_locals,
        )
        self.exc_type = exc_type
        self._str = _safe_string(exc_value, 'exception')
        try:
            self.__notes__ = getattr(exc_value, '__notes__', None)
        except Exception as e:
            self.__notes__ = [
                f"Ignored error getting __notes__: {_safe_string(e, '__notes__', repr)}"
            ]

        if exc_type and issubclass(exc_type, SyntaxError):
            self.filename = exc_value.filename
            lno = exc_value.lineno
            self.lineno = str(lno) if lno is not None else None
            end_lno = getattr(exc_value, 'end_lineno', None)
            self.end_lineno = str(end_lno) if end_lno is not None else None
            self.text = exc_value.text
            self.offset = exc_value.offset
            self.end_offset = getattr(exc_value, 'end_offset', None)
            self.msg = exc_value.msg

        if lookup_lines:
            self._load_lines()
        self.__suppress_context__ = (
            exc_value.__suppress_context__ if exc_value is not None else False
        )

        # Convert __cause__, __context__, and BaseExceptionGroup.exceptions
        # iteratively so deep chains do not overflow recursion depth.
        if not is_recursive_call:
            queue = [(self, exc_value)]
            while queue:
                te, e = queue.pop()
                if (
                    e is not None
                    and getattr(e, '__cause__', None) is not None
                    and id(e.__cause__) not in _seen
                ):
                    cause = TracebackException(
                        type(e.__cause__),
                        e.__cause__,
                        e.__cause__.__traceback__,
                        limit=limit,
                        lookup_lines=lookup_lines,
                        capture_locals=capture_locals,
                        max_group_width=max_group_width,
                        max_group_depth=max_group_depth,
                        _seen=_seen,
                    )
                else:
                    cause = None

                if compact:
                    need_context = (
                        cause is None
                        and e is not None
                        and not e.__suppress_context__
                    )
                else:
                    need_context = True

                if (
                    e is not None
                    and getattr(e, '__context__', None) is not None
                    and need_context
                    and id(e.__context__) not in _seen
                ):
                    context = TracebackException(
                        type(e.__context__),
                        e.__context__,
                        e.__context__.__traceback__,
                        limit=limit,
                        lookup_lines=lookup_lines,
                        capture_locals=capture_locals,
                        max_group_width=max_group_width,
                        max_group_depth=max_group_depth,
                        _seen=_seen,
                    )
                else:
                    context = None

                if e is not None and isinstance(e, BaseExceptionGroup):
                    exceptions = []
                    for exc in e.exceptions:
                        texc = TracebackException(
                            type(exc),
                            exc,
                            exc.__traceback__,
                            limit=limit,
                            lookup_lines=lookup_lines,
                            capture_locals=capture_locals,
                            max_group_width=max_group_width,
                            max_group_depth=max_group_depth,
                            _seen=_seen,
                        )
                        exceptions.append(texc)
                else:
                    exceptions = None

                te.__cause__ = cause
                te.__context__ = context
                te.exceptions = exceptions
                if cause:
                    queue.append((te.__cause__, e.__cause__))
                if context:
                    queue.append((te.__context__, e.__context__))
                if exceptions:
                    queue.extend(zip(te.exceptions, e.exceptions))

    @classmethod
    def from_exception(cls, exc, *args, **kwargs):
        """Create a TracebackException from an exception."""
        return cls(type(exc), exc, exc.__traceback__, *args, **kwargs)

    def _load_lines(self):
        for frame in self.stack:
            frame.line

    def __eq__(self, other):
        if isinstance(other, TracebackException):
            d1 = self.__dict__
            d2 = other.__dict__
            if len(d1) != len(d2):
                return False
            for k, v in d1.items():
                if k not in d2 or v != d2[k]:
                    return False
            return True
        return NotImplemented

    def __str__(self):
        return self._str

    def format_exception_only(self, *, show_group=False, _depth=0):
        """Format the exception part of the traceback."""
        indent = 3 * _depth * ' '
        if self.exc_type is None:
            yield indent + _format_final_exc_line(None, self._str)
            return

        stype = self.exc_type.__qualname__
        smod = self.exc_type.__module__
        if smod not in ("__main__", "builtins"):
            if not isinstance(smod, str):
                smod = "<unknown>"
            stype = smod + '.' + stype

        if not issubclass(self.exc_type, SyntaxError):
            yield indent + _format_final_exc_line(stype, self._str)
        else:
            yield from (indent + line for line in self._format_syntax_error(stype))

        if (
            isinstance(self.__notes__, collections.abc.Sequence)
            and not isinstance(self.__notes__, (str, bytes))
        ):
            for note in self.__notes__:
                note = _safe_string(note, 'note')
                yield from [indent + l + '\n' for l in note.split('\n')]
        elif self.__notes__ is not None:
            yield indent + "{}\n".format(
                _safe_string(self.__notes__, '__notes__', func=repr)
            )

        if show_group and self.exceptions is not None:
            for ex in self.exceptions:
                yield from ex.format_exception_only(
                    show_group=show_group, _depth=_depth + 1
                )

    def _format_syntax_error(self, stype):
        filename_suffix = ''
        if self.lineno is not None:
            yield '  File "{}", line {}\n'.format(
                self.filename or "<string>", self.lineno
            )
        elif self.filename is not None:
            filename_suffix = ' ({})'.format(self.filename)

        text = self.text
        if text is not None:
            rtext = text.rstrip('\n')
            ltext = rtext.lstrip(' \n\f')
            spaces = len(rtext) - len(ltext)
            yield '    {}\n'.format(ltext)

            if self.offset is not None:
                offset = self.offset
                end_offset = (
                    self.end_offset if self.end_offset not in {None, 0} else offset
                )
                if offset == end_offset or end_offset == -1:
                    end_offset = offset + 1

                colno = offset - 1 - spaces
                end_colno = end_offset - 1 - spaces
                if colno >= 0:
                    caretspace = (
                        (c if c.isspace() else ' ') for c in ltext[:colno]
                    )
                    yield '    {}{}'.format(
                        "".join(caretspace),
                        ('^' * (end_colno - colno) + "\n"),
                    )
        msg = self.msg or "<no detail available>"
        yield "{}: {}{}\n".format(stype, msg, filename_suffix)

    def format(self, *, chain=True, _ctx=None):
        """Format the exception."""
        if _ctx is None:
            _ctx = _ExceptionPrintContext()

        output = []
        exc = self
        if chain:
            while exc:
                if exc.__cause__ is not None:
                    chained_msg = _cause_message
                    chained_exc = exc.__cause__
                elif (
                    exc.__context__ is not None
                    and not exc.__suppress_context__
                ):
                    chained_msg = _context_message
                    chained_exc = exc.__context__
                else:
                    chained_msg = None
                    chained_exc = None

                output.append((chained_msg, exc))
                exc = chained_exc
        else:
            output.append((None, exc))

        for msg, exc in reversed(output):
            if msg is not None:
                yield from _ctx.emit(msg)
            if exc.exceptions is None:
                if exc.stack:
                    yield from _ctx.emit('Traceback (most recent call last):\n')
                    yield from _ctx.emit(exc.stack.format())
                yield from _ctx.emit(exc.format_exception_only())
            elif _ctx.exception_group_depth > self.max_group_depth:
                yield from _ctx.emit(
                    f"... (max_group_depth is {self.max_group_depth})\n"
                )
            else:
                is_toplevel = _ctx.exception_group_depth == 0
                if is_toplevel:
                    _ctx.exception_group_depth += 1

                if exc.stack:
                    yield from _ctx.emit(
                        'Exception Group Traceback (most recent call last):\n',
                        margin_char='+' if is_toplevel else None,
                    )
                    yield from _ctx.emit(exc.stack.format())

                yield from _ctx.emit(exc.format_exception_only())
                num_excs = len(exc.exceptions)
                if num_excs <= self.max_group_width:
                    n = num_excs
                else:
                    n = self.max_group_width + 1
                _ctx.need_close = False
                for i in range(n):
                    last_exc = i == n - 1
                    if last_exc:
                        _ctx.need_close = True

                    if self.max_group_width is not None:
                        truncated = i >= self.max_group_width
                    else:
                        truncated = False
                    title = f'{i + 1}' if not truncated else '...'
                    yield (
                        _ctx.indent()
                        + ('+-' if i == 0 else '  ')
                        + f'+---------------- {title} ----------------\n'
                    )
                    _ctx.exception_group_depth += 1
                    if not truncated:
                        yield from exc.exceptions[i].format(chain=chain, _ctx=_ctx)
                    else:
                        remaining = num_excs - self.max_group_width
                        plural = 's' if remaining > 1 else ''
                        yield from _ctx.emit(
                            f"and {remaining} more exception{plural}\n"
                        )

                    if last_exc and _ctx.need_close:
                        yield (
                            _ctx.indent()
                            + "+------------------------------------\n"
                        )
                        _ctx.need_close = False
                    _ctx.exception_group_depth -= 1

                if is_toplevel:
                    assert _ctx.exception_group_depth == 1
                    _ctx.exception_group_depth = 0

    def print(self, *, file=None, chain=True):
        """Print the result of self.format(chain=chain) to 'file'."""
        if file is None:
            file = sys.stderr
        for line in self.format(chain=chain):
            print(line, file=file, end="")
