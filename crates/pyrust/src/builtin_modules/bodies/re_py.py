"""
Regular-expression engine — pyrust port targeting CPython 3.12's ``re`` (issue
#2625).

A pure-Python recursive-descent compiler plus a backtracking matcher.  It is not
fast, but it is correct for the common surface: literals, ``.``, the
``\\d \\w \\s`` class shorthands (and negations), quantifiers (greedy and lazy),
groups (capturing / non-capturing / named), alternation, anchors, character
classes with ranges, and backreferences.

Public surface: ``match`` / ``fullmatch`` / ``search`` / ``findall`` /
``finditer`` / ``sub`` / ``subn`` / ``split`` / ``compile`` / ``escape`` plus the
``Pattern`` / ``Match`` objects and the ``error`` exception.

This source is exec'd once into a throwaway namespace at first ``import re`` and
the public names are copied onto the module by ``re.rs::inject_python_members``
(mirrors ``json`` / ``operator`` / ``string``).
"""

# --------------------------------------------------------------------------- #
# Flags
# --------------------------------------------------------------------------- #

A = ASCII = 256
I = IGNORECASE = 2
L = LOCALE = 4
M = MULTILINE = 8
S = DOTALL = 16
U = UNICODE = 32
X = VERBOSE = 64

# Single-letter inline flag characters, e.g. ``(?i)`` -> IGNORECASE.
_FLAG_LETTERS = {
    'i': IGNORECASE,
    'm': MULTILINE,
    's': DOTALL,
    'x': VERBOSE,
    'a': ASCII,
    'u': UNICODE,
    'L': LOCALE,
}

# --------------------------------------------------------------------------- #
# Exception
# --------------------------------------------------------------------------- #


class error(Exception):
    """Exception raised when a string passed to a function is not a valid
    regular expression, or when some other error occurs during compilation or
    matching."""

    def __init__(self, msg, pattern=None, pos=None):
        self.msg = msg
        self.pattern = pattern
        self.pos = pos
        if pos is not None and pattern is not None:
            # Best-effort line/column derivation (CPython does this too).
            nl = b'\n' if isinstance(pattern, (bytes, bytearray)) else '\n'
            self.lineno = pattern.count(nl, 0, pos) + 1
            self.colno = pos - pattern.rfind(nl, 0, pos)
            full = '%s at position %d' % (msg, pos)
        else:
            self.lineno = None
            self.colno = None
            full = msg
        super().__init__(full)


# --------------------------------------------------------------------------- #
# AST node kinds.  Each node is a tuple whose first element is a tag string.
# --------------------------------------------------------------------------- #
#   ('lit', ch)                  single literal character
#   ('any',)                     '.'
#   ('class', negated, items)    [...]  items: ('ch', c) / ('range', lo, hi) / ('cat', name)
#   ('cat', name)                \d \w \s \D \W \S
#   ('star', node, mode)         node* / node*? / node*+
#   ('plus', node, mode)         node+ / node+? / node++
#   ('opt', node, mode)          node? / node?? / node?+
#   ('repeat', node, lo, hi, mode)   node{lo,hi}  (hi None = inf)
#   ('group', idx, name, node)   ( ... )  idx None for non-capturing
#   ('atomic', node)             (?>...)
#   ('alt', branches)            a|b|c   branches: list of seq
#   ('seq', nodes)               concatenation
#   ('start',)                   ^
#   ('end',)                     $
#   ('bol_a',)                   \A
#   ('eol_z',)                   \Z
#   ('wordb', negated)           \b / \B
#   ('backref', idx, available)             \1 .. \99
#   ('backref_name', name, available)       (?P=name)
#   ('lookahead', positive, node)   (?=...) / (?!...)
#   ('lookbehind', positive, node, width)   (?<=...) / (?<!...) (fixed width)
#   ('scoped', flags, node)       (?i:...) / (?-i:...)

_HEX_DIGITS = frozenset('0123456789abcdefABCDEF')


class _Parser:
    def __init__(self, pattern, flags, is_bytes=False, orig_pattern=None):
        self.s = pattern
        self.orig_pattern = orig_pattern if orig_pattern is not None else pattern
        self.is_bytes = is_bytes
        self.i = 0
        self.n = len(pattern)
        self.flags = flags
        self.group_count = 0
        self.groupindex = {}
        # Completed capture bodies keyed by numeric group id.  Lookbehind
        # width analysis records the highest group id on entry, so
        # backreferences can use earlier fixed-width groups without admitting
        # forward, open, or same-lookbehind groups (issue #2803).
        self.group_nodes = {}
        # Nested lookbehinds inherit the outer assertion's boundary: a group
        # defined anywhere inside that outer assertion remains unavailable.
        self._lookbehind_group_limit = None
        # Tracks whether any real atom has been parsed yet; global inline
        # flags ``(?i)`` are only valid before the first atom (CPython rule).
        self._seen_atom = False

    def _err(self, msg, pos=None):
        if pos is None:
            pos = self.i
        raise error(msg, self.orig_pattern, pos)

    def parse(self):
        node = self._parse_alt()
        if self.i != self.n:
            # An unmatched ')' is the usual cause.
            self._err("unbalanced parenthesis")
        return node

    def _parse_alt(self):
        branches = [self._parse_seq()]
        while self.i < self.n and self.s[self.i] == '|':
            self.i += 1
            branches.append(self._parse_seq())
        if len(branches) == 1:
            return branches[0]
        return ('alt', branches)

    def _parse_seq(self):
        nodes = []
        while self.i < self.n:
            c = self.s[self.i]
            if c == '|' or c == ')':
                break
            if self.flags & VERBOSE and (c.isspace() or c == '#'):
                if c == '#':
                    while self.i < self.n and self.s[self.i] != '\n':
                        self.i += 1
                else:
                    self.i += 1
                continue
            atom = self._parse_atom()
            atom = self._parse_quant(atom)
            nodes.append(atom)
        return ('seq', nodes)

    def _parse_quant(self, atom):
        if self.i >= self.n:
            return atom
        c = self.s[self.i]
        quant = None
        if c == '*':
            self.i += 1
            mode = self._consume_modifier()
            quant = ('star', atom, mode)
        elif c == '+':
            self.i += 1
            mode = self._consume_modifier()
            quant = ('plus', atom, mode)
        elif c == '?':
            self.i += 1
            mode = self._consume_modifier()
            quant = ('opt', atom, mode)
        elif c == '{':
            saved = self.i
            rep = self._try_parse_brace()
            if rep is None:
                # Literal '{' — restore and treat as a literal char.
                self.i = saved
                return atom
            lo, hi = rep
            mode = self._consume_modifier()
            quant = ('repeat', atom, lo, hi, mode)
        if quant is not None:
            if self.i < self.n and self.s[self.i] in '*+?':
                self._err("multiple repeat")
            return quant
        return atom

    def _consume_modifier(self):
        # Returns quantifier mode: 'lazy' ('?'), 'possessive' ('+'), or 'greedy'.
        if self.i < self.n:
            ch = self.s[self.i]
            if ch == '?':
                self.i += 1
                return 'lazy'
            if ch == '+':
                self.i += 1
                return 'possessive'
        return 'greedy'

    def _try_parse_brace(self):
        # self.i points at '{'. Returns (lo, hi) or None if not a valid brace.
        j = self.i + 1
        lo_digits = ''
        while j < self.n and self.s[j].isdigit():
            lo_digits += self.s[j]
            j += 1
        hi = None
        has_comma = False
        if j < self.n and self.s[j] == ',':
            has_comma = True
            j += 1
            hi_digits = ''
            while j < self.n and self.s[j].isdigit():
                hi_digits += self.s[j]
                j += 1
        if j >= self.n or self.s[j] != '}':
            return None
        if not lo_digits and not has_comma:
            return None
        self.i = j + 1
        lo = int(lo_digits) if lo_digits else 0
        if has_comma:
            hi = int(hi_digits) if hi_digits else None
        else:
            hi = lo
        if hi is not None and hi < lo:
            self._err("min repeat greater than max repeat")
        return (lo, hi)

    def _parse_atom(self):
        c = self.s[self.i]
        if c == '(':
            # _parse_group sets _seen_atom itself (a global ``(?i)`` flag
            # group must not count as an atom).
            return self._parse_group()
        self._seen_atom = True
        if c == '[':
            return self._parse_class()
        if c == '.':
            self.i += 1
            return ('any',)
        if c == '^':
            self.i += 1
            return ('start',)
        if c == '$':
            self.i += 1
            return ('end',)
        if c == '\\':
            return self._parse_escape()
        if c == '*' or c == '+' or c == '?':
            self._err("nothing to repeat")
        self.i += 1
        return ('lit', c)

    def _parse_group(self):
        self.i += 1  # consume '('
        if self.i < self.n and self.s[self.i] == '?':
            self.i += 1
            if self.i >= self.n:
                self._err("unexpected end of pattern")
            kind = self.s[self.i]
            if kind in _FLAG_LETTERS or kind == '-':
                return self._parse_flag_group()
            if kind == '#':
                # A comment is not an atom and may precede global flags.
                self.i += 1
                while self.i < self.n and self.s[self.i] != ')':
                    self.i += 1
                self._expect(')')
                return ('seq', [])
            self._seen_atom = True
            if kind == ':':
                self.i += 1
                node = self._parse_alt()
                self._expect(')')
                return ('group', None, None, node)
            if kind == '>':
                self.i += 1
                node = self._parse_alt()
                self._expect(')')
                return ('atomic', node)
            if kind == 'P':
                self.i += 1
                if self.i < self.n and self.s[self.i] == '<':
                    self.i += 1
                    name = self._read_group_name('>')
                    idx = self._register_group(name)
                    node = self._parse_alt()
                    self._expect(')')
                    self.group_nodes[idx] = node
                    return ('group', idx, name, node)
                if self.i < self.n and self.s[self.i] == '=':
                    self.i += 1
                    name = self._read_group_name(')')
                    idx = self.groupindex.get(name)
                    available = (idx is not None
                                 and idx in self.group_nodes)
                    return ('backref_name', name, available)
                self._err("unknown extension ?P")
            if kind == '=' or kind == '!':
                self.i += 1
                node = self._parse_alt()
                self._expect(')')
                return ('lookahead', kind == '=', node)
            if kind == '<':
                self.i += 1
                if self.i >= self.n:
                    self._err("unexpected end of pattern")
                sub = self.s[self.i]
                if sub == '=' or sub == '!':
                    self.i += 1
                    previous_limit = self._lookbehind_group_limit
                    max_group = (self.group_count if previous_limit is None
                                 else previous_limit)
                    self._lookbehind_group_limit = max_group
                    node = self._parse_alt()
                    self._expect(')')
                    self._lookbehind_group_limit = previous_limit
                    width = _fixed_width(node, self.group_nodes,
                                         self.groupindex, max_group)
                    if width < 0:
                        raise error("look-behind requires fixed-width pattern")
                    return ('lookbehind', sub == '=', node, width)
                self._err("unknown extension ?<" + sub)
            self._err("unknown extension ?" + kind)
        # plain capturing group
        self._seen_atom = True
        idx = self._register_group(None)
        node = self._parse_alt()
        self._expect(')')
        self.group_nodes[idx] = node
        return ('group', idx, None, node)

    def _parse_flag_group(self):
        # self.i points at the first flag letter or '-' after '(?'.
        # Handles global flags ``(?i)`` and scoped flag groups
        # ``(?i:...)`` / ``(?i-m:...)`` / ``(?-m:...)``.
        start = self.i
        add = 0
        while self.i < self.n and self.s[self.i] in _FLAG_LETTERS:
            add |= _FLAG_LETTERS[self.s[self.i]]
            self.i += 1
        if self.i >= self.n:
            self._err("missing -, : or )")
        c = self.s[self.i]
        if c not in ')-:':
            # A stray flag-letter-like char: alphabetic chars are an
            # unrecognised flag, anything else is a structural error.
            self._err("unknown flag" if c.isalpha() else "missing -, : or )")
        if c == ')':
            # Global flags: only valid before any atom, applied pattern-wide.
            if self._seen_atom:
                self._err("global flags not at the start of the expression",
                          start - 2)
            if self.is_bytes and (add & UNICODE):
                raise ValueError(
                    "cannot use UNICODE flag with a bytes pattern")
            if not self.is_bytes and (add & LOCALE):
                raise ValueError(
                    "cannot use LOCALE flag with a str pattern")
            self.i += 1
            self.flags |= add
            return ('seq', [])
        remove = 0
        if c == '-':
            self.i += 1
            rstart = self.i
            while self.i < self.n and self.s[self.i] in _FLAG_LETTERS:
                remove |= _FLAG_LETTERS[self.s[self.i]]
                self.i += 1
            if self.i == rstart:
                # No flag letters were consumed after '-'.
                if self.i < self.n and self.s[self.i].isalpha():
                    self._err("unknown flag")
                self._err("missing flag")
            if remove & (ASCII | UNICODE | LOCALE):
                self._err("bad inline flags: cannot turn off flags "
                          "'a', 'u' and 'L'")
            if self.i >= self.n or self.s[self.i] != ':':
                self._err("unknown flag"
                          if self.i < self.n and self.s[self.i].isalpha()
                          else "missing :")
        # c == ':' (possibly after a '-flags' clause)
        if add & remove:
            # The same flag is both turned on and off.
            self._err("bad inline flags: flag turned on and off")
        self.i += 1  # consume ':'
        self._seen_atom = True
        saved = self.flags
        self.flags = (self.flags | add) & ~remove
        node = self._parse_alt()
        scoped_flags = self.flags
        self.flags = saved
        self._expect(')')
        return ('scoped', scoped_flags, node)

    def _register_group(self, name):
        self.group_count += 1
        idx = self.group_count
        if name is not None:
            if name in self.groupindex:
                self._err("redefinition of group name %r" % name)
            self.groupindex[name] = idx
        return idx

    def _read_group_name(self, terminator):
        name = ''
        while self.i < self.n and self.s[self.i] != terminator:
            name += self.s[self.i]
            self.i += 1
        if self.i >= self.n:
            self._err("missing %r, unterminated name" % terminator)
        self.i += 1  # consume terminator
        if not name:
            self._err("missing group name")
        return name

    def _expect(self, ch):
        if self.i >= self.n or self.s[self.i] != ch:
            if ch == ')':
                self._err("missing ), unterminated subpattern")
            self._err("expected %r" % ch)
        self.i += 1

    def _parse_class(self):
        self.i += 1  # consume '['
        negated = False
        if self.i < self.n and self.s[self.i] == '^':
            negated = True
            self.i += 1
        items = []
        # A ']' as the first char is a literal.
        if self.i < self.n and self.s[self.i] == ']':
            items.append(('ch', ']'))
            self.i += 1
        while self.i < self.n and self.s[self.i] != ']':
            c = self.s[self.i]
            if c == '\\':
                esc_start = self.i
                self.i += 1
                if self.i >= self.n:
                    self._err("bad escape (end of pattern)")
                esc = self.s[self.i]
                self.i += 1
                cat = _CLASS_ESCAPES.get(esc)
                if cat is not None:
                    items.append(('cat', cat))
                    continue
                lo = self._decode_class_escape_at(esc, esc_start)
                if (self.i < self.n and self.s[self.i] == '-'
                        and self.i + 1 < self.n and self.s[self.i + 1] != ']'):
                    self.i += 1
                    hi = self._read_class_member_char()
                    if ord(hi) < ord(lo):
                        self._err("bad character range %s-%s" % (lo, hi))
                    items.append(('range', lo, hi))
                else:
                    items.append(('ch', lo))
                continue
            # literal, maybe a range
            self.i += 1
            if (self.i < self.n and self.s[self.i] == '-'
                    and self.i + 1 < self.n and self.s[self.i + 1] != ']'):
                self.i += 1
                hi = self._read_class_member_char()
                if ord(hi) < ord(c):
                    self._err("bad character range %s-%s" % (c, hi))
                items.append(('range', c, hi))
            else:
                items.append(('ch', c))
        if self.i >= self.n:
            self._err("unterminated character set")
        self.i += 1  # consume ']'
        return ('class', negated, items)

    def _read_class_member_char(self):
        c = self.s[self.i]
        if c == '\\':
            esc_start = self.i
            self.i += 1
            if self.i >= self.n:
                self._err("bad escape (end of pattern)")
            esc = self.s[self.i]
            self.i += 1
            return self._decode_class_escape_at(esc, esc_start)
        self.i += 1
        return c

    def _parse_hex_escape(self, esc, esc_start):
        if esc == 'x':
            count = 2
        elif esc == 'u':
            if self.is_bytes:
                self._err("bad escape \\u", esc_start)
            count = 4
        elif esc == 'U':
            if self.is_bytes:
                self._err("bad escape \\U", esc_start)
            count = 8
        else:
            return None
        digits = self.s[self.i:self.i + count]
        if len(digits) < count or any(ch not in _HEX_DIGITS for ch in digits):
            self._err("incomplete escape \\%s%s" % (esc, digits), esc_start)
        self.i += count
        cp = int(digits, 16)
        if cp > 0x10FFFF:
            self._err("bad escape \\%s%s" % (esc, digits), esc_start)
        return chr(cp)

    def _decode_class_escape_at(self, esc, esc_start):
        if esc in ('x', 'u', 'U'):
            return self._parse_hex_escape(esc, esc_start)
        if esc == '0':
            return self._parse_octal_escape(esc)
        return _decode_class_escape(esc)

    def _parse_octal_escape(self, first):
        oct_digits = first
        while (self.i < self.n and len(oct_digits) < 3
               and '0' <= self.s[self.i] <= '7'):
            oct_digits += self.s[self.i]
            self.i += 1
        cp = int(oct_digits, 8)
        if cp > 0o377:
            self._err("octal escape value \\%s outside of range 0-0o377" % oct_digits)
        return chr(cp)

    def _parse_escape(self):
        esc_start = self.i
        self.i += 1  # consume '\'
        if self.i >= self.n:
            self._err("bad escape (end of pattern)")
        c = self.s[self.i]
        self.i += 1
        if c in _CLASS_ESCAPES:
            return ('cat', _CLASS_ESCAPES[c])
        if c == 'b':
            return ('wordb', False)
        if c == 'B':
            return ('wordb', True)
        if c == 'A':
            return ('bol_a',)
        if c == 'Z':
            return ('eol_z',)
        if c in ('x', 'u', 'U'):
            return ('lit', self._parse_hex_escape(c, esc_start))
        if c == '0':
            return ('lit', self._parse_octal_escape(c))
        if c.isdigit():
            # Three octal digits \100..\377 denote a character literal;
            # otherwise 1-2 digits denote a backreference \1..\99.
            if (self.i + 1 < self.n
                    and '0' <= c <= '7'
                    and '0' <= self.s[self.i] <= '7'
                    and '0' <= self.s[self.i + 1] <= '7'):
                return ('lit', self._parse_octal_escape(c))
            num = c
            while self.i < self.n and self.s[self.i].isdigit() and len(num) < 2:
                num += self.s[self.i]
                self.i += 1
            idx = int(num)
            return ('backref', idx, idx in self.group_nodes)
        return ('lit', _decode_simple_escape(c))


_CLASS_ESCAPES = {
    'd': 'd', 'D': 'D',
    'w': 'w', 'W': 'W',
    's': 's', 'S': 'S',
}

_SIMPLE_ESCAPES = {
    'n': '\n', 't': '\t', 'r': '\r', 'f': '\f', 'v': '\v', 'a': '\a',
    '0': '\0',
}


def _decode_simple_escape(c):
    if c in _SIMPLE_ESCAPES:
        return _SIMPLE_ESCAPES[c]
    return c


def _decode_class_escape(c):
    # Inside a character class, ``\b`` is the backspace character (it is the
    # word-boundary assertion only outside a class).
    if c == 'b':
        return '\b'
    if c in _SIMPLE_ESCAPES:
        return _SIMPLE_ESCAPES[c]
    return c


# --------------------------------------------------------------------------- #
# Matcher — backtracking via continuation-passing.
# --------------------------------------------------------------------------- #


_INVALID_WIDTH = -2


def _fixed_width(node, group_nodes=None, groupindex=None, max_group=None,
                 resolving=(), width_cache=None):
    """Return the constant character width matched by ``node``.

    ``-1`` denotes a variable width and ``_INVALID_WIDTH`` an unavailable or
    cyclic group reference.  Keeping those cases distinct lets an exact zero
    repeat discard a variable-width body without hiding an invalid reference.

    ``group_nodes`` contains completed capture bodies and ``max_group`` is the
    highest id registered before the lookbehind began.  Backreferences resolve
    recursively within that boundary.
    """
    if width_cache is None:
        width_cache = {}
    tag = node[0]
    if tag == 'lit' or tag == 'any' or tag == 'cat' or tag == 'class':
        return 1
    if tag == 'backref' or tag == 'backref_name':
        if group_nodes is None:
            return _INVALID_WIDTH
        if not node[2]:
            return _INVALID_WIDTH
        if tag == 'backref':
            idx = node[1]
        else:
            idx = groupindex.get(node[1]) if groupindex is not None else None
        if (idx is None or idx not in group_nodes or idx in resolving
                or (max_group is not None and idx > max_group)):
            return _INVALID_WIDTH
        if idx in width_cache:
            return width_cache[idx]
        width = _fixed_width(group_nodes[idx], group_nodes, groupindex,
                             max_group, resolving + (idx,), width_cache)
        width_cache[idx] = width
        return width
    if tag in ('start', 'end', 'bol_a', 'eol_z', 'wordb'):
        return 0
    if tag == 'lookahead' or tag == 'lookbehind':
        width = _fixed_width(node[2], group_nodes, groupindex, max_group,
                             resolving, width_cache)
        return _INVALID_WIDTH if width == _INVALID_WIDTH else 0
    if tag == 'seq':
        total = 0
        variable = False
        for child in node[1]:
            width = _fixed_width(child, group_nodes, groupindex, max_group,
                                 resolving, width_cache)
            if width >= 0:
                total += width
            elif width == _INVALID_WIDTH:
                return _INVALID_WIDTH
            else:
                variable = True
        return -1 if variable else total
    if tag == 'group':
        return _fixed_width(node[3], group_nodes, groupindex, max_group,
                            resolving, width_cache)
    if tag == 'atomic':
        return _fixed_width(node[1], group_nodes, groupindex, max_group,
                            resolving, width_cache)
    if tag == 'scoped':
        return _fixed_width(node[2], group_nodes, groupindex, max_group,
                            resolving, width_cache)
    if tag == 'atomic':
        return _fixed_width(node[1], group_nodes, groupindex, max_group,
                            resolving, width_cache)
    if tag == 'alt':
        width = None
        variable = False
        for branch in node[1]:
            branch_width = _fixed_width(branch, group_nodes, groupindex,
                                        max_group, resolving, width_cache)
            if branch_width >= 0:
                if width is None:
                    width = branch_width
                elif width != branch_width:
                    variable = True
            elif branch_width == _INVALID_WIDTH:
                return _INVALID_WIDTH
            else:
                variable = True
        if variable:
            return -1
        return width if width is not None else 0
    if tag == 'opt' or tag == 'star' or tag == 'plus':
        width = _fixed_width(node[1], group_nodes, groupindex, max_group,
                             resolving, width_cache)
        if width == _INVALID_WIDTH:
            return _INVALID_WIDTH
        return 0 if width == 0 else -1
    if tag == 'repeat':
        lo = node[2]
        hi = node[3]
        width = _fixed_width(node[1], group_nodes, groupindex, max_group,
                             resolving, width_cache)
        if width == _INVALID_WIDTH:
            return _INVALID_WIDTH
        if hi is None or lo != hi:
            return 0 if width == 0 else -1
        if lo == 0:
            return 0
        if width < 0:
            return -1
        return width * lo
    return -1


def _is_word(ch, ascii_only=False):
    if ascii_only:
        return ch == '_' or ('a' <= ch <= 'z') or ('A' <= ch <= 'Z') or ('0' <= ch <= '9')
    return ch == '_' or ch.isalnum()


def _ascii_lower(ch):
    if 'A' <= ch <= 'Z':
        return chr(ord(ch) + 32)
    return ch


def _ascii_upper(ch):
    if 'a' <= ch <= 'z':
        return chr(ord(ch) - 32)
    return ch


def _ascii_lower_str(s):
    return ''.join(_ascii_lower(ch) for ch in s)


def _cat_matches(name, ch, ascii_only):
    if name == 'd':
        return ('0' <= ch <= '9') if ascii_only else ch.isdigit()
    if name == 'D':
        return not _cat_matches('d', ch, ascii_only)
    if name == 'w':
        return _is_word(ch, ascii_only)
    if name == 'W':
        return not _cat_matches('w', ch, ascii_only)
    if name == 's':
        if ascii_only:
            return ch in ' \t\n\r\f\v'
        return ch.isspace()
    if name == 'S':
        return not _cat_matches('s', ch, ascii_only)
    return False


class _Matcher:
    def __init__(self, node, text, flags, ngroups, groupindex, is_bytes=False):
        self.node = node
        self.text = text
        self.n = len(text)
        self.flags = flags
        self.is_bytes = is_bytes
        self.ignorecase = bool(flags & IGNORECASE)
        self.multiline = bool(flags & MULTILINE)
        self.dotall = bool(flags & DOTALL)
        self.ascii_only = bool(flags & ASCII) or is_bytes
        self.ngroups = ngroups
        self.groupindex = groupindex
        self.groups = None

    def match_at(self, start, require_end=None):
        # `require_end` (fullmatch) forces the overall match to land exactly at
        # that position.  Returning None from `accept` on a too-short match lets
        # the backtracker keep exploring longer alternatives (e.g. the `ab`
        # branch of `a|ab` for fullmatch) instead of locking in the first
        # success.
        self.groups = [None] * (self.ngroups + 1)

        if require_end is None:
            def accept(pos):
                return pos
        else:
            def accept(pos):
                if pos == require_end:
                    return pos
                return None

        res = self._m(self.node, start, accept)
        if res is None:
            return None
        return (start, res, [g for g in self.groups])

    def _eqch(self, a, b):
        if a == b:
            return True
        if self.ignorecase:
            if self.is_bytes:
                return _ascii_lower(a) == _ascii_lower(b)
            return a.lower() == b.lower()
        return False

    def _m(self, node, pos, cont):
        tag = node[0]
        if tag == 'seq':
            return self._m_seq(node[1], 0, pos, cont)
        if tag == 'lit':
            ch = node[1]
            if pos < self.n and self._eqch(self.text[pos], ch):
                return cont(pos + 1)
            return None
        if tag == 'any':
            if pos < self.n and (self.dotall or self.text[pos] != '\n'):
                return cont(pos + 1)
            return None
        if tag == 'cat':
            if pos < self.n and _cat_matches(node[1], self.text[pos], self.ascii_only):
                return cont(pos + 1)
            return None
        if tag == 'class':
            if pos < self.n and self._class_match(node[1], node[2], self.text[pos]):
                return cont(pos + 1)
            return None
        if tag == 'start':
            if pos == 0 or (self.multiline and pos > 0 and self.text[pos - 1] == '\n'):
                return cont(pos)
            return None
        if tag == 'end':
            if pos == self.n:
                return cont(pos)
            if self.multiline and pos < self.n and self.text[pos] == '\n':
                return cont(pos)
            if pos == self.n - 1 and self.text[pos] == '\n':
                return cont(pos)
            return None
        if tag == 'bol_a':
            if pos == 0:
                return cont(pos)
            return None
        if tag == 'eol_z':
            if pos == self.n:
                return cont(pos)
            return None
        if tag == 'wordb':
            negated = node[1]
            before = pos > 0 and _is_word(self.text[pos - 1], self.ascii_only)
            after = pos < self.n and _is_word(self.text[pos], self.ascii_only)
            at_boundary = before != after
            if at_boundary != negated:
                return cont(pos)
            return None
        if tag == 'scoped':
            return self._m_scoped(node, pos, cont)
        if tag == 'group':
            return self._m_group(node, pos, cont)
        if tag == 'atomic':
            saved = list(self.groups)
            end = self._m(node[1], pos, lambda p: p)
            if end is None:
                self.groups = saved
                return None
            r = cont(end)
            if r is None:
                self.groups = saved
            return r
        if tag == 'alt':
            for branch in node[1]:
                r = self._m(branch, pos, cont)
                if r is not None:
                    return r
            return None
        if tag == 'star':
            return self._m_repeat(node[1], 0, None, node[2], pos, cont)
        if tag == 'plus':
            return self._m_repeat(node[1], 1, None, node[2], pos, cont)
        if tag == 'opt':
            return self._m_repeat(node[1], 0, 1, node[2], pos, cont)
        if tag == 'repeat':
            return self._m_repeat(node[1], node[2], node[3], node[4], pos, cont)
        if tag == 'backref':
            return self._m_backref(node[1], pos, cont)
        if tag == 'backref_name':
            idx = self.groupindex.get(node[1])
            return self._m_backref(idx, pos, cont)
        if tag == 'lookahead':
            positive = node[1]
            saved = list(self.groups)
            matched = self._m(node[2], pos, lambda p: p) is not None
            if not positive:
                self.groups = saved
            if matched == positive:
                r = cont(pos)
                if r is not None:
                    return r
            self.groups = saved
            return None
        if tag == 'atomic':
            saved = list(self.groups)
            end = self._m(node[1], pos, lambda p: p)
            if end is None:
                self.groups = saved
                return None
            r = cont(end)
            if r is None:
                self.groups = saved
            return r
        if tag == 'lookbehind':
            positive = node[1]
            width = node[3]
            start = pos - width
            saved = list(self.groups)
            if start < 0:
                matched = False
            else:
                matched = self._m(
                    node[2], start,
                    lambda p: p if p == pos else None) is not None
            if not positive:
                self.groups = saved
            if matched == positive:
                r = cont(pos)
                if r is not None:
                    return r
            self.groups = saved
            return None
        return None

    def _m_seq(self, nodes, idx, pos, cont):
        if idx == len(nodes):
            return cont(pos)

        def next_cont(p):
            return self._m_seq(nodes, idx + 1, p, cont)

        return self._m(nodes[idx], pos, next_cont)

    def _m_scoped(self, node, pos, cont):
        # ('scoped', flags, subnode): the flag overrides apply only to the
        # enclosed subpattern; the continuation runs under the outer flags.
        flags = node[1]
        sub = node[2]
        outer = (self.ignorecase, self.multiline, self.dotall,
                 self.ascii_only)

        def set_flags(ic, ml, da, ao):
            self.ignorecase = ic
            self.multiline = ml
            self.dotall = da
            self.ascii_only = ao

        def restore_cont(end):
            set_flags(*outer)
            r = cont(end)
            if r is None:
                # Re-enter the scope for further backtracking inside `sub`.
                set_flags(bool(flags & IGNORECASE), bool(flags & MULTILINE),
                          bool(flags & DOTALL), bool(flags & ASCII) or self.is_bytes)
            return r

        set_flags(bool(flags & IGNORECASE), bool(flags & MULTILINE),
                  bool(flags & DOTALL), bool(flags & ASCII) or self.is_bytes)
        r = self._m(sub, pos, restore_cont)
        set_flags(*outer)
        return r

    def _m_group(self, node, pos, cont):
        idx = node[1]
        sub = node[3]
        if idx is None:
            return self._m(sub, pos, cont)
        saved = self.groups[idx]

        def group_cont(end):
            prev = self.groups[idx]
            self.groups[idx] = (pos, end)
            r = cont(end)
            if r is None:
                self.groups[idx] = prev
            return r

        r = self._m(sub, pos, group_cont)
        if r is None:
            self.groups[idx] = saved
        return r

    def _m_repeat(self, node, lo, hi, mode, pos, cont):
        # Match `node` between lo and hi times (hi None = unbounded).
        if mode == 'possessive':
            saved = list(self.groups)
            p = pos
            count = 0
            while hi is None or count < hi:
                prev_groups = list(self.groups)
                np = self._m(node, p, lambda end: end)
                if np is None:
                    self.groups = prev_groups
                    break
                if np == p and count >= lo:
                    # Stop looping on a zero-width match once minimum `lo` is
                    # satisfied (retaining group updates from this iteration to
                    # match CPython 3.12).
                    break
                p = np
                count += 1
            if count < lo:
                self.groups = saved
                return None
            r = cont(p)
            if r is None:
                self.groups = saved
            return r

        greedy = (mode == 'greedy' or mode is True)

        def rec(count, p):
            can_more = hi is None or count < hi
            if greedy:
                if can_more:
                    def after_one(np):
                        if np == p:
                            return None  # zero-width guard
                        return rec(count + 1, np)
                    r = self._m(node, p, after_one)
                    if r is not None:
                        return r
                if count >= lo:
                    return cont(p)
                return None
            else:
                if count >= lo:
                    r = cont(p)
                    if r is not None:
                        return r
                if can_more:
                    def after_one(np):
                        if np == p:
                            return None
                        return rec(count + 1, np)
                    return self._m(node, p, after_one)
                return None

        return rec(0, pos)

    def _m_backref(self, idx, pos, cont):
        if idx is None or idx >= len(self.groups) or self.groups[idx] is None:
            return None
        gs, ge = self.groups[idx]
        sub = self.text[gs:ge]
        ln = len(sub)
        if self.ignorecase:
            if self.is_bytes:
                if _ascii_lower_str(self.text[pos:pos + ln]) == _ascii_lower_str(sub):
                    return cont(pos + ln)
                return None
            if self.text[pos:pos + ln].lower() == sub.lower():
                return cont(pos + ln)
            return None
        if self.text[pos:pos + ln] == sub:
            return cont(pos + ln)
        return None

    def _class_match(self, negated, items, ch):
        matched = False
        for it in items:
            kind = it[0]
            if kind == 'ch':
                if self._eqch(ch, it[1]):
                    matched = True
                    break
            elif kind == 'range':
                lo = it[1]
                hi = it[2]
                if lo <= ch <= hi:
                    matched = True
                    break
                if self.ignorecase:
                    if self.is_bytes:
                        cl = _ascii_lower(ch)
                        cu = _ascii_upper(ch)
                    else:
                        cl = ch.lower()
                        cu = ch.upper()
                    if (lo <= cl <= hi) or (lo <= cu <= hi):
                        matched = True
                        break
            elif kind == 'cat':
                if _cat_matches(it[1], ch, self.ascii_only):
                    matched = True
                    break
        if negated:
            return not matched
        return matched


# --------------------------------------------------------------------------- #
# Match object
# --------------------------------------------------------------------------- #


class Match:
    def __init__(self, pattern, text, start, end, groups, pos, endpos):
        self.re = pattern
        self.string = text
        self._start = start
        self._end = end
        self._groups = groups  # list indexed 1..ngroups of (s,e)|None
        self.pos = pos
        self.endpos = endpos
        self.lastindex = None
        self.lastgroup = None
        last = None
        for i in range(1, len(groups)):
            if groups[i] is not None:
                last = i
        self.lastindex = last
        if last is not None:
            for name, idx in pattern.groupindex.items():
                if idx == last:
                    self.lastgroup = name

    def _resolve(self, key):
        if isinstance(key, int):
            return key
        idx = self.re.groupindex.get(key)
        if idx is None:
            raise IndexError("no such group")
        return idx

    def group(self, *args):
        if len(args) == 0:
            return self._get(0)
        if len(args) == 1:
            return self._get(self._resolve(args[0]))
        return tuple(self._get(self._resolve(a)) for a in args)

    def _slice(self, start, end):
        part = self.string[start:end]
        if self.re._is_bytes and isinstance(part, bytearray):
            return bytes(part)
        return part

    def _get(self, idx):
        if idx == 0:
            return self._slice(self._start, self._end)
        if idx < 0 or idx >= len(self._groups):
            raise IndexError("no such group")
        span = self._groups[idx]
        if span is None:
            return None
        return self._slice(span[0], span[1])

    def __getitem__(self, key):
        return self._get(self._resolve(key))

    def groups(self, default=None):
        out = []
        for i in range(1, len(self._groups)):
            span = self._groups[i]
            if span is None:
                out.append(default)
            else:
                out.append(self._slice(span[0], span[1]))
        return tuple(out)

    def groupdict(self, default=None):
        out = {}
        for name, idx in self.re.groupindex.items():
            span = self._groups[idx]
            if span is None:
                out[name] = default
            else:
                out[name] = self._slice(span[0], span[1])
        return out

    def expand(self, template):
        return _expand_template(template, self)

    def start(self, group=0):
        return self._span(group)[0]

    def end(self, group=0):
        return self._span(group)[1]

    def span(self, group=0):
        return self._span(group)

    def _span(self, group):
        idx = self._resolve(group)
        if idx == 0:
            return (self._start, self._end)
        if idx < 0 or idx >= len(self._groups):
            raise IndexError("no such group")
        span = self._groups[idx]
        if span is None:
            return (-1, -1)
        return (span[0], span[1])

    def __repr__(self):
        return "<re.Match object; span=(%d, %d), match=%r>" % (
            self._start, self._end, self._slice(self._start, self._end))


# --------------------------------------------------------------------------- #
# Pattern object
# --------------------------------------------------------------------------- #


def _is_bytes_like(obj):
    return isinstance(obj, (bytes, bytearray))


class Pattern:
    def __init__(self, pattern, flags):
        if isinstance(pattern, str):
            if flags & LOCALE:
                raise ValueError("cannot use LOCALE flag with a str pattern")
            if (flags & ASCII) and (flags & UNICODE):
                raise ValueError("ASCII and UNICODE flags are incompatible")
            self._is_bytes = False
            self.pattern = pattern
            str_pattern = pattern
        elif _is_bytes_like(pattern):
            if flags & UNICODE:
                raise ValueError("cannot use UNICODE flag with a bytes pattern")
            if (flags & ASCII) and (flags & LOCALE):
                raise ValueError("ASCII and LOCALE flags are incompatible")
            self._is_bytes = True
            self.pattern = pattern
            str_pattern = pattern.decode('latin-1')
        else:
            raise TypeError("first argument must be string or compiled pattern")
        self.flags = flags
        parser = _Parser(str_pattern, flags, is_bytes=self._is_bytes,
                         orig_pattern=pattern)
        self._ast = parser.parse()
        # Global inline flags ``(?i)`` update the parser's flags in place;
        # pick them up so the matcher sees the effective pattern-wide flags.
        self.flags = parser.flags
        self.groups = parser.group_count
        self.groupindex = parser.groupindex

    def _prepare_subject(self, string):
        if self._is_bytes:
            if _is_bytes_like(string):
                return string.decode('latin-1')
            if isinstance(string, str):
                raise TypeError(
                    "cannot use a bytes pattern on a string-like object")
            raise TypeError(
                "expected string or bytes-like object, got %r"
                % type(string).__name__)
        if isinstance(string, str):
            return string
        if _is_bytes_like(string):
            raise TypeError(
                "cannot use a string pattern on a bytes-like object")
        raise TypeError(
            "expected string or bytes-like object, got %r"
            % type(string).__name__)

    def _match_at(self, text, start, require_end=None):
        m = _Matcher(self._ast, text, self.flags, self.groups, self.groupindex,
                     is_bytes=self._is_bytes)
        return m.match_at(start, require_end)

    def match(self, string, pos=0, endpos=None):
        text = self._prepare_subject(string)
        if endpos is None:
            endpos = len(text)
        res = self._match_at(text[:endpos], pos)
        if res is None:
            return None
        s, e, groups = res
        return Match(self, string, s, e, groups, pos, endpos)

    def fullmatch(self, string, pos=0, endpos=None):
        text = self._prepare_subject(string)
        if endpos is None:
            endpos = len(text)
        sub = text[:endpos]
        res = self._match_at(sub, pos, endpos)
        if res is None:
            return None
        s, e, groups = res
        return Match(self, string, s, e, groups, pos, endpos)

    def search(self, string, pos=0, endpos=None):
        text = self._prepare_subject(string)
        if endpos is None:
            endpos = len(text)
        sub = text[:endpos]
        i = pos
        while i <= endpos:
            res = self._match_at(sub, i)
            if res is not None:
                s, e, groups = res
                return Match(self, string, s, e, groups, pos, endpos)
            i += 1
        return None

    def finditer(self, string, pos=0, endpos=None):
        text = self._prepare_subject(string)
        if endpos is None:
            endpos = len(text)
        sub = text[:endpos]
        # Preserve the eager range/type check that _findall_matches performed
        # before returning its list iterator.
        past_end = pos > endpos
        return self._iter_matches(string, sub, pos, endpos, past_end)

    def _iter_matches(self, string, sub, pos, endpos, past_end):
        if past_end:
            return
        i = pos
        while i <= endpos:
            res = self._match_at(sub, i)
            if res is None:
                i += 1
                continue
            s, e, groups = res
            yield Match(self, string, s, e, groups, pos, endpos)
            if e == i:
                i += 1
            else:
                i = e

    def _findall_matches(self, string, pos=0, endpos=None):
        text = self._prepare_subject(string)
        if endpos is None:
            endpos = len(text)
        sub = text[:endpos]
        out = []
        i = pos
        while i <= endpos:
            res = self._match_at(sub, i)
            if res is None:
                i += 1
                continue
            s, e, groups = res
            out.append(Match(self, string, s, e, groups, pos, endpos))
            if e == i:
                i += 1
            else:
                i = e
        return out

    def findall(self, string, pos=0, endpos=None):
        matches = self._findall_matches(string, pos, endpos)
        empty = b'' if self._is_bytes else ''
        out = []
        for m in matches:
            if self.groups == 0:
                out.append(m.group(0))
            elif self.groups == 1:
                g = m.group(1)
                out.append(g if g is not None else empty)
            else:
                out.append(m.groups(empty))
        return out

    def sub(self, repl, string, count=0):
        return self.subn(repl, string, count)[0]

    def subn(self, repl, string, count=0):
        matches = self._findall_matches(string)
        src = bytes(string) if (self._is_bytes and isinstance(string, bytearray)) else string
        empty = b'' if self._is_bytes else ''
        out = []
        last = 0
        n = 0
        for m in matches:
            if count and n >= count:
                break
            out.append(src[last:m._start])
            out.append(_expand(repl, m))
            last = m._end
            n += 1
        out.append(src[last:])
        return (empty.join(out), n)

    def split(self, string, maxsplit=0):
        matches = self._findall_matches(string)
        src = bytes(string) if (self._is_bytes and isinstance(string, bytearray)) else string
        out = []
        last = 0
        n = 0
        for m in matches:
            if maxsplit and n >= maxsplit:
                break
            out.append(src[last:m._start])
            if self.groups:
                for gi in range(1, self.groups + 1):
                    span = m._groups[gi]
                    if span is None:
                        out.append(None)
                    else:
                        out.append(src[span[0]:span[1]])
            last = m._end
            n += 1
        out.append(src[last:])
        return out

    def __eq__(self, other):
        if not isinstance(other, Pattern):
            return NotImplemented
        return (self._is_bytes == other._is_bytes
                and self.pattern == other.pattern
                and self.flags == other.flags)

    def __hash__(self):
        pat = bytes(self.pattern) if isinstance(self.pattern, bytearray) else self.pattern
        return hash((type(self.pattern), pat, self.flags))

    def __repr__(self):
        return "re.compile(%r)" % self.pattern


# --------------------------------------------------------------------------- #
# Replacement template expansion
# --------------------------------------------------------------------------- #

_TEMPLATE_ESCAPES = {
    'a': '\a',
    'b': '\b',
    'f': '\f',
    'n': '\n',
    'r': '\r',
    't': '\t',
    'v': '\v',
    '\\': '\\',
}


def _expand_template(template, match):
    is_bytes = match.re._is_bytes
    if is_bytes:
        if not _is_bytes_like(template):
            if isinstance(template, str):
                raise TypeError(
                    "cannot use a string pattern on a bytes-like object")
            raise TypeError(
                "expected bytes-like object, got %r" % type(template).__name__)
        repl = template.decode('latin-1')
    else:
        if not isinstance(template, str):
            if _is_bytes_like(template):
                raise TypeError(
                    "cannot use a bytes pattern on a string-like object")
            raise TypeError(
                "expected string, got %r" % type(template).__name__)
        repl = template

    out = []
    i = 0
    n = len(repl)
    while i < n:
        c = repl[i]
        if c != '\\':
            out.append(c)
            i += 1
            continue
        esc_pos = i
        i += 1
        if i >= n:
            raise error("bad escape (end of pattern)", template, esc_pos)
        d = repl[i]
        if d == '0':
            i += 1
            oct_digits = '0'
            while i < n and len(oct_digits) < 3 and '0' <= repl[i] <= '7':
                oct_digits += repl[i]
                i += 1
            out.append(chr(int(oct_digits, 8)))
        elif d.isdigit():
            # 3 octal digits \100..\377 denote a character literal;
            # 1-2 digits denote a group backreference.
            if (i + 2 < n
                    and '0' <= d <= '7'
                    and '0' <= repl[i + 1] <= '7'
                    and '0' <= repl[i + 2] <= '7'):
                cp = int(repl[i:i + 3], 8)
                if cp > 0o377:
                    raise error(
                        "octal escape value \\%s outside of range 0-0o377"
                        % repl[i:i + 3], template, esc_pos)
                out.append(chr(cp))
                i += 3
            else:
                num = d
                i += 1
                while i < n and repl[i].isdigit() and len(num) < 2:
                    num += repl[i]
                    i += 1
                gidx = int(num)
                if gidx > match.re.groups:
                    raise error(
                        "invalid group reference %d" % gidx,
                        template,
                        esc_pos + 1,
                    )
                g = match.group(gidx)
                if g is not None:
                    out.append(g.decode('latin-1') if is_bytes else g)
        elif d == 'g':
            i += 1
            if i >= n or repl[i] != '<':
                raise error("missing <", template, i)
            i += 1
            name = ''
            while i < n and repl[i] != '>':
                name += repl[i]
                i += 1
            if i >= n:
                raise error("missing >, unterminated name", template, esc_pos)
            i += 1
            stripped = name.strip()
            if not stripped:
                raise error("missing group name", template, esc_pos)
            if stripped.isdigit() or (stripped.startswith('+') and stripped[1:].isdigit()):
                gidx = int(stripped)
                if gidx > match.re.groups:
                    raise error(
                        "invalid group reference %d" % gidx,
                        template,
                        esc_pos + 3,
                    )
                g = match.group(gidx)
            elif stripped.startswith('-') and stripped[1:].isdigit():
                raise error("negative group number", template, esc_pos)
            else:
                g = match.group(name)
            if g is not None:
                out.append(g.decode('latin-1') if is_bytes else g)
        elif d in _TEMPLATE_ESCAPES:
            out.append(_TEMPLATE_ESCAPES[d])
            i += 1
        elif ('a' <= d <= 'z') or ('A' <= d <= 'Z'):
            raise error("bad escape \\%s" % d, template, esc_pos)
        else:
            out.append('\\')
            out.append(d)
            i += 1
    res = ''.join(out)
    return res.encode('latin-1') if is_bytes else res


def _expand(repl, match):
    if callable(repl):
        res = repl(match)
        if match.re._is_bytes:
            if isinstance(res, bytearray):
                return bytes(res)
            if not isinstance(res, bytes):
                raise TypeError(
                    "expected bytes-like object, got %r" % type(res).__name__)
        elif not isinstance(res, str):
            raise TypeError("expected string, got %r" % type(res).__name__)
        return res
    return _expand_template(repl, match)


# --------------------------------------------------------------------------- #
# Module-level cache + public API
# --------------------------------------------------------------------------- #

_cache = {}


def compile(pattern, flags=0):
    if isinstance(pattern, Pattern):
        if flags:
            raise ValueError(
                "cannot process flags argument with a compiled pattern")
        return pattern
    if isinstance(pattern, bytearray):
        key = (bytearray, bytes(pattern), flags)
    else:
        key = (type(pattern), pattern, flags)
    cached = _cache.get(key)
    if cached is not None:
        return cached
    p = Pattern(pattern, flags)
    if len(_cache) >= 512:
        _cache.clear()
    _cache[key] = p
    return p


def match(pattern, string, flags=0):
    return compile(pattern, flags).match(string)


def fullmatch(pattern, string, flags=0):
    return compile(pattern, flags).fullmatch(string)


def search(pattern, string, flags=0):
    return compile(pattern, flags).search(string)


def findall(pattern, string, flags=0):
    return compile(pattern, flags).findall(string)


def finditer(pattern, string, flags=0):
    return compile(pattern, flags).finditer(string)


def sub(pattern, repl, string, count=0, flags=0):
    return compile(pattern, flags).sub(repl, string, count)


def subn(pattern, repl, string, count=0, flags=0):
    return compile(pattern, flags).subn(repl, string, count)


def split(pattern, string, maxsplit=0, flags=0):
    return compile(pattern, flags).split(string, maxsplit)


_SPECIAL_CHARS = set('()[]{}?*+-|^$\\.&~# \t\n\r\v\f\x00')
_SPECIAL_BYTES = set(b'()[]{}?*+-|^$\\.&~# \t\n\r\v\f\x00')


def escape(pattern):
    if _is_bytes_like(pattern):
        out = bytearray()
        for b in pattern:
            if b in _SPECIAL_BYTES:
                if b == 0:
                    out.extend(b'\\000')
                    continue
                out.append(92)  # ord('\\')
            out.append(b)
        return bytes(out)
    out = []
    for c in pattern:
        if c in _SPECIAL_CHARS:
            if c == '\x00':
                out.append('\\000')
                continue
            out.append('\\')
        out.append(c)
    return ''.join(out)


def purge():
    _cache.clear()
