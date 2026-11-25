"""Tokenization help for Python programs (CPython 3.12)."""

import builtins
import collections
import io
import re
import token
from token import *
from token import EXACT_TOKEN_TYPES

__all__ = token.__all__ + [
    'tokenize',
    'generate_tokens',
    'detect_encoding',
    'untokenize',
    'TokenInfo',
]

_builtin_open = builtins.open

cookie_re = re.compile(r'^[ \t\f]*#.*?coding[:=][ \t]*([-\w.]+)', re.ASCII)
blank_re = re.compile(rb'^[ \t\f]*(?:[#\r\n]|$)', re.ASCII)

_UTF8_BOM = b'\xef\xbb\xbf'

# Known Python codec names and aliases for detect_encoding validation.
_ENCODING_ALIASES = {
    'utf-8': 'utf-8',
    'utf_8': 'utf-8',
    'utf8': 'utf-8',
    'u8': 'utf-8',
    'utf': 'utf-8',
    'cp65001': 'utf-8',
    'utf-8-sig': 'utf-8-sig',
    'utf_8_sig': 'utf-8-sig',
    'ascii': 'ascii',
    'us-ascii': 'ascii',
    '646': 'ascii',
    'latin-1': 'iso-8859-1',
    'latin_1': 'iso-8859-1',
    'latin1': 'iso-8859-1',
    'latin': 'iso-8859-1',
    'l1': 'iso-8859-1',
    'iso-8859-1': 'iso-8859-1',
    'iso_8859_1': 'iso-8859-1',
    'iso8859-1': 'iso-8859-1',
    'iso8859_1': 'iso-8859-1',
    '8859': 'iso-8859-1',
    'cp819': 'iso-8859-1',
    'utf-16': 'utf-16',
    'utf_16': 'utf-16',
    'utf16': 'utf-16',
    'utf-16-le': 'utf-16-le',
    'utf_16_le': 'utf-16-le',
    'utf-16-be': 'utf-16-be',
    'utf_16_be': 'utf-16-be',
    'utf-32': 'utf-32',
    'utf_32': 'utf-32',
    'utf32': 'utf-32',
    'utf-32-le': 'utf-32-le',
    'utf_32_le': 'utf-32-le',
    'utf-32-be': 'utf-32-be',
    'utf_32_be': 'utf-32-be',
    'cp1252': 'cp1252',
    'windows-1252': 'cp1252',
    'cp1250': 'cp1250',
    'cp1251': 'cp1251',
    'cp437': 'cp437',
    'cp850': 'cp850',
    'iso-8859-15': 'iso-8859-15',
    'iso8859-15': 'iso-8859-15',
    'iso-8859-2': 'iso-8859-2',
    'iso8859-2': 'iso-8859-2',
    'shift_jis': 'shift_jis',
    'shift-jis': 'shift_jis',
    'sjis': 'shift_jis',
    'cp932': 'cp932',
    'euc_jp': 'euc_jp',
    'euc-jp': 'euc_jp',
    'gbk': 'gbk',
    'gb2312': 'gb2312',
    'gb18030': 'gb18030',
    'big5': 'big5',
    'koi8-r': 'koi8-r',
    'koi8_r': 'koi8-r',
    'mac_roman': 'mac_roman',
    'mac-roman': 'mac_roman',
}


class TokenInfo(collections.namedtuple('TokenInfo', 'type string start end line')):
    def __repr__(self):
        annotated_type = '%d (%s)' % (self.type, tok_name[self.type])
        return (
            f'TokenInfo(type={annotated_type}, string={self.string!r}, '
            f'start={self.start!r}, end={self.end!r}, line={self.line!r})'
        )

    @property
    def exact_type(self):
        if self.type == OP and self.string in EXACT_TOKEN_TYPES:
            return EXACT_TOKEN_TYPES[self.string]
        else:
            return self.type


class TokenError(Exception):
    pass


class StopTokenizing(Exception):
    pass


def _get_normal_name(orig_enc):
    """Imitates get_normal_name in tokenizer.c."""
    enc = orig_enc[:12].lower().replace('_', '-')
    if enc == 'utf-8' or enc.startswith('utf-8-'):
        return 'utf-8'
    if enc in ('latin-1', 'iso-8859-1', 'iso-latin-1') or enc.startswith(
        ('latin-1-', 'iso-8859-1-', 'iso-latin-1-')
    ):
        return 'iso-8859-1'
    return orig_enc


def _lookup_encoding(name):
    key = name.lower().strip()
    if key in _ENCODING_ALIASES:
        return _ENCODING_ALIASES[key]
    norm = key.replace('_', '-')
    if norm in _ENCODING_ALIASES:
        return _ENCODING_ALIASES[norm]
    # Try decoding empty bytes to test if the runtime supports it directly.
    try:
        b''.decode(name)
        return name.lower().replace('_', '-')
    except LookupError:
        return None


def detect_encoding(readline):
    """
    Detect the encoding that should be used to decode a Python source file.
    Follows PEP 263 and handles UTF-8 BOM.
    """
    try:
        filename = readline.__self__.name
    except AttributeError:
        filename = None
    bom_found = False
    encoding = None
    default = 'utf-8'

    def read_or_stop():
        try:
            return readline()
        except StopIteration:
            return b''

    def find_cookie(line):
        try:
            line_string = line.decode('utf-8')
        except UnicodeDecodeError:
            msg = 'invalid or missing encoding declaration'
            if filename is not None:
                msg = '{} for {!r}'.format(msg, filename)
            raise SyntaxError(msg)

        match = cookie_re.match(line_string)
        if not match:
            return None
        enc = _get_normal_name(match.group(1))
        resolved = _lookup_encoding(enc)
        if resolved is None:
            if filename is None:
                msg = 'unknown encoding: ' + enc
            else:
                msg = 'unknown encoding for {!r}: {}'.format(filename, enc)
            raise SyntaxError(msg)

        if bom_found:
            if enc != 'utf-8':
                if filename is None:
                    msg = 'encoding problem: utf-8'
                else:
                    msg = 'encoding problem for {!r}: utf-8'.format(filename)
                raise SyntaxError(msg)
            enc += '-sig'
        return enc

    first = read_or_stop()
    if first.startswith(_UTF8_BOM):
        bom_found = True
        first = first[3:]
        default = 'utf-8-sig'
    if not first:
        return default, []

    encoding = find_cookie(first)
    if encoding:
        return encoding, [first]
    if not blank_re.match(first):
        return default, [first]

    second = read_or_stop()
    if not second:
        return default, [first]

    encoding = find_cookie(second)
    if encoding:
        return encoding, [first, second]

    return default, [first, second]


def _raise_lex_error(res):
    err_type = res[1]
    msg = res[2]
    line = res[3]
    col = res[4]
    text = res[5]
    if err_type == 'TokenError':
        raise TokenError(msg, (line, col))
    if err_type == 'IndentationError':
        raise IndentationError(msg, ('<tokenize>', line, col, text))
    if err_type == 'TabError':
        raise TabError(msg, ('<tokenize>', line, col, text))
    raise TokenError(msg, (line, col))


def generate_tokens(readline):
    """Tokenize a source reading Python code as unicode strings."""
    handle = _lexer_new()
    while True:
        try:
            line = readline()
        except StopIteration:
            line = ''
        if not line:
            res = _lexer_finish(handle)
            if not res[0]:
                _raise_lex_error(res)
            for item in res[1]:
                yield TokenInfo(*item)
            return
        res = _lexer_feed_line(handle, line)
        if not res[0]:
            _raise_lex_error(res)
        for item in res[1]:
            yield TokenInfo(*item)


def tokenize(readline):
    """Tokenize a source reading Python code as bytes."""
    encoding, consumed = detect_encoding(readline)
    empty = b''
    rl_gen = iter(consumed)

    def _next_line():
        for chunk in rl_gen:
            return chunk
        try:
            return readline()
        except StopIteration:
            return empty

    yield TokenInfo(ENCODING, encoding, (0, 0), (0, 0), '')

    def str_readline():
        raw = _next_line()
        if not raw:
            return ''
        return raw.decode(encoding)

    yield from generate_tokens(str_readline)


def open(filename):
    """Open a file in read only mode using the encoding detected by detect_encoding()."""
    buffer = _builtin_open(filename, 'rb')
    try:
        encoding, lines = detect_encoding(buffer.readline)
        buffer.close()
        return _builtin_open(filename, 'r', encoding=encoding)
    except Exception:
        buffer.close()
        raise


class Untokenizer:
    def __init__(self):
        self.tokens = []
        self.prev_row = 1
        self.prev_col = 0
        self.prev_type = None
        self.prev_line = ''
        self.encoding = None

    def add_whitespace(self, start):
        row, col = start
        if row < self.prev_row or row == self.prev_row and col < self.prev_col:
            raise ValueError(
                'start ({},{}) precedes previous end ({},{})'.format(
                    row, col, self.prev_row, self.prev_col
                )
            )
        self.add_backslash_continuation(start)
        col_offset = col - self.prev_col
        if col_offset:
            self.tokens.append(' ' * col_offset)

    def add_backslash_continuation(self, start):
        row = start[0]
        row_offset = row - self.prev_row
        if row_offset:
            self.tokens.append('\\\n' * row_offset)
            self.prev_col = 0

    def escape_brackets(self, token):
        characters = []
        consume_until_next_bracket = False
        for character in token:
            if character == '}' and consume_until_next_bracket:
                consume_until_next_bracket = False
            else:
                if character in '{}':
                    characters.append(character)
                characters.append(character)
                if character == '{':
                    consume_until_next_bracket = False
        return ''.join(characters)

    def untokenize(self, iterable):
        it = iter(iterable)
        indents = []
        startline = False
        for t in it:
            if len(t) == 2:
                self.compat(t, it)
                break
            tok_type, token, start, end, line = t
            if tok_type == ENCODING:
                self.encoding = token
                continue
            if tok_type == ENDMARKER:
                break
            if tok_type == INDENT:
                indents.append(token)
                continue
            elif tok_type == DEDENT:
                indents.pop()
                self.prev_row, self.prev_col = end
                continue
            elif tok_type in (NEWLINE, NL):
                startline = True
            elif startline and indents:
                indent = indents[-1]
                if start[1] >= len(indent):
                    self.tokens.append(indent)
                    self.prev_col = len(indent)
                startline = False
            elif tok_type in {FSTRING_MIDDLE, FSTRING_END}:
                if '{' in token or '}' in token:
                    token = self.escape_brackets(token)
                    last_line = token.splitlines()[-1]
                    end_line, end_col = end
                    extra_chars = last_line.count('{{') + last_line.count('}}')
                    end = (end_line, end_col + extra_chars)

            self.add_whitespace(start)
            self.tokens.append(token)
            self.prev_row, self.prev_col = end
            if tok_type in (NEWLINE, NL):
                self.prev_row += 1
                self.prev_col = 0
            self.prev_type = tok_type
            self.prev_line = line
        return ''.join(self.tokens)

    def compat(self, token, iterable):
        indents = []
        toks_append = self.tokens.append
        startline = token[0] in (NEWLINE, NL)
        prevstring = False
        in_fstring = 0

        for tok in _chain_one(token, iterable):
            toknum, tokval = tok[:2]
            if toknum == ENCODING:
                self.encoding = tokval
                continue

            if toknum in (NAME, NUMBER):
                tokval += ' '

            if toknum == STRING:
                if prevstring:
                    tokval = ' ' + tokval
                prevstring = True
            else:
                prevstring = False

            if toknum == FSTRING_START:
                in_fstring += 1
            elif toknum == FSTRING_END:
                in_fstring -= 1
            if toknum == INDENT:
                indents.append(tokval)
                continue
            elif toknum == DEDENT:
                indents.pop()
                continue
            elif toknum in (NEWLINE, NL):
                startline = True
            elif startline and indents:
                toks_append(indents[-1])
                startline = False
            elif toknum == FSTRING_MIDDLE:
                if '{' in tokval or '}' in tokval:
                    tokval = self.escape_brackets(tokval)

            toks_append(tokval)


def _chain_one(first, rest):
    yield first
    yield from rest


def untokenize(iterable):
    """Transform tokens back into Python source code."""
    ut = Untokenizer()
    out = ut.untokenize(iterable)
    if ut.encoding is not None:
        out = out.encode(ut.encoding)
    return out
