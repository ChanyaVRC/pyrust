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
M = MULTILINE = 8
S = DOTALL = 16
X = VERBOSE = 64

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
            self.lineno = pattern.count('\n', 0, pos) + 1
            self.colno = pos - pattern.rfind('\n', 0, pos)
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
#   ('star', node, greedy)       node*
#   ('plus', node, greedy)       node+
#   ('opt', node, greedy)        node?
#   ('repeat', node, lo, hi, greedy)   node{lo,hi}  (hi None = inf)
#   ('group', idx, name, node)   ( ... )  idx None for non-capturing
#   ('alt', branches)            a|b|c   branches: list of seq
#   ('seq', nodes)               concatenation
#   ('start',)                   ^
#   ('end',)                     $
#   ('bol_a',)                   \A
#   ('eol_z',)                   \Z
#   ('wordb', negated)           \b / \B
#   ('backref', idx)             \1 .. \99
#   ('backref_name', name)       (?P=name)
#   ('lookahead', positive, node)   (?=...) / (?!...)


class _Parser:
    def __init__(self, pattern, flags):
        self.s = pattern
        self.i = 0
        self.n = len(pattern)
        self.flags = flags
        self.group_count = 0
        self.groupindex = {}

    def _err(self, msg, pos=None):
        if pos is None:
            pos = self.i
        raise error(msg, self.s, pos)

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
        if c == '*':
            self.i += 1
            greedy = self._consume_lazy()
            return ('star', atom, greedy)
        if c == '+':
            self.i += 1
            greedy = self._consume_lazy()
            return ('plus', atom, greedy)
        if c == '?':
            self.i += 1
            greedy = self._consume_lazy()
            return ('opt', atom, greedy)
        if c == '{':
            saved = self.i
            rep = self._try_parse_brace()
            if rep is None:
                # Literal '{' — restore and treat as a literal char.
                self.i = saved
                return atom
            lo, hi = rep
            greedy = self._consume_lazy()
            return ('repeat', atom, lo, hi, greedy)
        return atom

    def _consume_lazy(self):
        # Returns greedy flag; consumes a trailing '?' (lazy) if present.
        if self.i < self.n and self.s[self.i] == '?':
            self.i += 1
            return False
        return True

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
            return self._parse_group()
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
            if kind == ':':
                self.i += 1
                node = self._parse_alt()
                self._expect(')')
                return ('group', None, None, node)
            if kind == 'P':
                self.i += 1
                if self.i < self.n and self.s[self.i] == '<':
                    self.i += 1
                    name = self._read_group_name('>')
                    idx = self._register_group(name)
                    node = self._parse_alt()
                    self._expect(')')
                    return ('group', idx, name, node)
                if self.i < self.n and self.s[self.i] == '=':
                    self.i += 1
                    name = self._read_group_name(')')
                    return ('backref_name', name)
                self._err("unknown extension ?P")
            if kind == '#':
                self.i += 1
                while self.i < self.n and self.s[self.i] != ')':
                    self.i += 1
                self._expect(')')
                return ('seq', [])
            if kind == '=' or kind == '!':
                self.i += 1
                node = self._parse_alt()
                self._expect(')')
                return ('lookahead', kind == '=', node)
            self._err("unknown extension ?" + kind)
        # plain capturing group
        idx = self._register_group(None)
        node = self._parse_alt()
        self._expect(')')
        return ('group', idx, None, node)

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
                self.i += 1
                if self.i >= self.n:
                    self._err("bad escape (end of pattern)")
                esc = self.s[self.i]
                self.i += 1
                cat = _CLASS_ESCAPES.get(esc)
                if cat is not None:
                    items.append(('cat', cat))
                    continue
                lo = _decode_class_escape(esc)
                if (self.i < self.n and self.s[self.i] == '-'
                        and self.i + 1 < self.n and self.s[self.i + 1] != ']'):
                    self.i += 1
                    hi = self._read_class_member_char()
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
            self.i += 1
            if self.i >= self.n:
                self._err("bad escape (end of pattern)")
            esc = self.s[self.i]
            self.i += 1
            return _decode_class_escape(esc)
        self.i += 1
        return c

    def _parse_escape(self):
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
        if c.isdigit() and c != '0':
            num = c
            while self.i < self.n and self.s[self.i].isdigit() and len(num) < 2:
                num += self.s[self.i]
                self.i += 1
            return ('backref', int(num))
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


def _is_word(ch):
    return ch == '_' or ch.isalnum()


def _cat_matches(name, ch, ascii_only):
    if name == 'd':
        return ('0' <= ch <= '9') if ascii_only else ch.isdigit()
    if name == 'D':
        return not _cat_matches('d', ch, ascii_only)
    if name == 'w':
        if ascii_only:
            return ch == '_' or ('a' <= ch <= 'z') or ('A' <= ch <= 'Z') or ('0' <= ch <= '9')
        return _is_word(ch)
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
    def __init__(self, node, text, flags, ngroups, groupindex):
        self.node = node
        self.text = text
        self.n = len(text)
        self.flags = flags
        self.ignorecase = bool(flags & IGNORECASE)
        self.multiline = bool(flags & MULTILINE)
        self.dotall = bool(flags & DOTALL)
        self.ascii_only = bool(flags & ASCII)
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
            before = pos > 0 and _is_word(self.text[pos - 1])
            after = pos < self.n and _is_word(self.text[pos])
            at_boundary = before != after
            if at_boundary != negated:
                return cont(pos)
            return None
        if tag == 'group':
            return self._m_group(node, pos, cont)
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
                return cont(pos)
            self.groups = saved
            return None
        return None

    def _m_seq(self, nodes, idx, pos, cont):
        if idx == len(nodes):
            return cont(pos)

        def next_cont(p):
            return self._m_seq(nodes, idx + 1, p, cont)

        return self._m(nodes[idx], pos, next_cont)

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

    def _m_repeat(self, node, lo, hi, greedy, pos, cont):
        # Match `node` between lo and hi times (hi None = unbounded).
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

    def _get(self, idx):
        if idx == 0:
            return self.string[self._start:self._end]
        if idx < 0 or idx >= len(self._groups):
            raise IndexError("no such group")
        span = self._groups[idx]
        if span is None:
            return None
        return self.string[span[0]:span[1]]

    def __getitem__(self, key):
        return self._get(self._resolve(key))

    def groups(self, default=None):
        out = []
        for i in range(1, len(self._groups)):
            span = self._groups[i]
            if span is None:
                out.append(default)
            else:
                out.append(self.string[span[0]:span[1]])
        return tuple(out)

    def groupdict(self, default=None):
        out = {}
        for name, idx in self.re.groupindex.items():
            span = self._groups[idx]
            if span is None:
                out[name] = default
            else:
                out[name] = self.string[span[0]:span[1]]
        return out

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
            self._start, self._end, self.string[self._start:self._end])


# --------------------------------------------------------------------------- #
# Pattern object
# --------------------------------------------------------------------------- #


class Pattern:
    def __init__(self, pattern, flags):
        self.pattern = pattern
        self.flags = flags
        parser = _Parser(pattern, flags)
        self._ast = parser.parse()
        self.groups = parser.group_count
        self.groupindex = parser.groupindex

    def _match_at(self, text, start, require_end=None):
        m = _Matcher(self._ast, text, self.flags, self.groups, self.groupindex)
        return m.match_at(start, require_end)

    def match(self, string, pos=0, endpos=None):
        if endpos is None:
            endpos = len(string)
        res = self._match_at(string[:endpos], pos)
        if res is None:
            return None
        s, e, groups = res
        return Match(self, string, s, e, groups, pos, endpos)

    def fullmatch(self, string, pos=0, endpos=None):
        if endpos is None:
            endpos = len(string)
        sub = string[:endpos]
        res = self._match_at(sub, pos, endpos)
        if res is None:
            return None
        s, e, groups = res
        return Match(self, string, s, e, groups, pos, endpos)

    def search(self, string, pos=0, endpos=None):
        if endpos is None:
            endpos = len(string)
        sub = string[:endpos]
        i = pos
        while i <= endpos:
            res = self._match_at(sub, i)
            if res is not None:
                s, e, groups = res
                return Match(self, string, s, e, groups, pos, endpos)
            i += 1
        return None

    def finditer(self, string, pos=0, endpos=None):
        return iter(self._findall_matches(string, pos, endpos))

    def _findall_matches(self, string, pos=0, endpos=None):
        if endpos is None:
            endpos = len(string)
        sub = string[:endpos]
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
        out = []
        for m in matches:
            if self.groups == 0:
                out.append(m.group(0))
            elif self.groups == 1:
                g = m.group(1)
                out.append(g if g is not None else '')
            else:
                out.append(m.groups(''))
        return out

    def sub(self, repl, string, count=0):
        return self.subn(repl, string, count)[0]

    def subn(self, repl, string, count=0):
        matches = self._findall_matches(string)
        out = []
        last = 0
        n = 0
        for m in matches:
            if count and n >= count:
                break
            out.append(string[last:m._start])
            out.append(_expand(repl, m))
            last = m._end
            n += 1
        out.append(string[last:])
        return (''.join(out), n)

    def split(self, string, maxsplit=0):
        matches = self._findall_matches(string)
        out = []
        last = 0
        n = 0
        for m in matches:
            if maxsplit and n >= maxsplit:
                break
            out.append(string[last:m._start])
            if self.groups:
                for gi in range(1, self.groups + 1):
                    span = m._groups[gi]
                    if span is None:
                        out.append(None)
                    else:
                        out.append(string[span[0]:span[1]])
            last = m._end
            n += 1
        out.append(string[last:])
        return out

    def __repr__(self):
        return "re.compile(%r)" % self.pattern


# --------------------------------------------------------------------------- #
# Replacement template expansion
# --------------------------------------------------------------------------- #


def _expand(repl, match):
    if callable(repl):
        return repl(match)
    out = []
    i = 0
    n = len(repl)
    while i < n:
        c = repl[i]
        if c != '\\':
            out.append(c)
            i += 1
            continue
        i += 1
        if i >= n:
            raise error("bad escape (end of pattern)")
        d = repl[i]
        if d.isdigit():
            num = d
            i += 1
            while i < n and repl[i].isdigit() and len(num) < 2:
                num += repl[i]
                i += 1
            g = match.group(int(num))
            out.append(g if g is not None else '')
        elif d == 'g':
            i += 1
            if i >= n or repl[i] != '<':
                raise error("missing <")
            i += 1
            name = ''
            while i < n and repl[i] != '>':
                name += repl[i]
                i += 1
            if i >= n:
                raise error("missing >, unterminated name")
            i += 1
            if name.isdigit():
                g = match.group(int(name))
            else:
                g = match.group(name)
            out.append(g if g is not None else '')
        elif d == 'n':
            out.append('\n')
            i += 1
        elif d == 't':
            out.append('\t')
            i += 1
        elif d == 'r':
            out.append('\r')
            i += 1
        elif d == '\\':
            out.append('\\')
            i += 1
        else:
            out.append('\\')
            out.append(d)
            i += 1
    return ''.join(out)


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
    key = (pattern, flags)
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


def escape(pattern):
    special = set('()[]{}?*+-|^$\\.&~# \t\n\r\v\f')
    out = []
    for c in pattern:
        if c in special:
            out.append('\\')
        out.append(c)
    return ''.join(out)


def purge():
    _cache.clear()
