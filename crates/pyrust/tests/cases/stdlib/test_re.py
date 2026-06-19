# Parity coverage for the `re` module (issue #2625).
#
# A pure-Python regex engine ported in `re_py.py`.  These cases exercise the
# minimum viable surface: matching, searching, findall/finditer, sub/subn,
# split, compiled patterns, flags, groups (positional + named), anchors,
# quantifiers (greedy + lazy), character classes, backreferences, and the
# `re.error` exception.  All output is version-stable across CPython 3.11/3.12.

import re

# --- Basic matching ---------------------------------------------------------
m = re.match(r'(\d+)', '123abc')
print(m.group(0))
print(m.group(1))
print(m.start())
print(m.end())
print(m.span())

m2 = re.search(r'\d+', 'abc123def')
print(m2.group())
print(m2.start())
print(m2.span())

# No match -> None
print(re.match(r'\d+', 'abc'))
print(re.search(r'zzz', 'abc'))

# --- fullmatch --------------------------------------------------------------
print(bool(re.fullmatch(r'\d+', '123')))
print(bool(re.fullmatch(r'\d+', '12a')))

# --- findall ----------------------------------------------------------------
print(re.findall(r'\d+', 'a1b22c333'))
print(re.findall(r'(\w)(\d)', 'a1 b2 c3'))
print(re.findall(r'(\d)', '1 2 3'))
print(re.findall(r'[a-c]+', 'xaybczc'))

# --- finditer ---------------------------------------------------------------
print([mm.group() for mm in re.finditer(r'\d+', 'a12b345c6')])
print([mm.span() for mm in re.finditer(r'\d+', 'a12b345')])

# --- sub / subn -------------------------------------------------------------
print(re.sub(r'\d+', 'X', 'a1b22c333'))
print(re.sub(r'(\d+)', r'[\1]', 'a1b22'))
print(re.sub(r'(?P<n>\d+)', r'\g<n>!', 'a1b22'))
print(re.subn(r'\d', 'N', 'a1b2c3'))
print(re.sub(r'\d', 'N', 'a1b2c3', count=2))


def _upper(m):
    return m.group().upper()


print(re.sub(r'[a-z]+', _upper, 'ab cd'))

# --- split ------------------------------------------------------------------
print(re.split(r'\s+', 'a b  c'))
print(re.split(r'(\d)', 'a1b2c'))
print(re.split(r',', 'a,b,c', maxsplit=1))

# --- compile + methods ------------------------------------------------------
pat = re.compile(r'\d+')
print(pat.findall('a1b22'))
print(pat.match('123abc').group())
print(pat.pattern)
print(pat.groups)

named = re.compile(r'(?P<year>\d{4})-(?P<month>\d{2})')
mm = named.match('2024-06')
print(mm.group('year'), mm.group('month'))
print(mm.groupdict())
print(mm.groups())
print(named.groupindex)

# --- flags ------------------------------------------------------------------
print(re.match(r'hello', 'HELLO', re.IGNORECASE).group())
print(bool(re.search(r'^bar', 'foo\nbar', re.MULTILINE)))
print(re.search(r'a.c', 'a\nc', re.DOTALL).group())

# --- quantifiers ------------------------------------------------------------
print(re.match(r'a{2,3}', 'aaaa').group())
print(re.match(r'a{2}', 'aaaa').group())
print(re.match(r'colou?r', 'color').group())
print(re.match(r'colou?r', 'colour').group())
print(re.match(r'\d+?', '123').group())
print(re.match(r'<.*?>', '<a><b>').group())

# --- anchors / word boundary ------------------------------------------------
print(re.search(r'\bword\b', 'a word here').group())
print(bool(re.search(r'\bword\b', 'awordhere')))

# --- alternation + non-capturing --------------------------------------------
print(re.findall(r'cat|dog', 'a cat and a dog'))
print(re.match(r'(?:ab)+', 'ababab').group())

# --- backreference in pattern -----------------------------------------------
print(bool(re.search(r'(\w)\1', 'hello')))
print(bool(re.search(r'(\w)\1', 'abc')))

# --- optional / missing groups ----------------------------------------------
print(re.match(r'(a)(b)?', 'a').groups())
print(re.match(r'(a)(b)?', 'ab').groups())

# --- escape -----------------------------------------------------------------
print(re.escape('a.b*c+d'))
print(re.match(re.escape('a.b'), 'a.b').group())

# --- re.error ---------------------------------------------------------------
try:
    re.compile(r'[')
except re.error as e:
    print(type(e).__name__)

try:
    re.compile(r'(')
except re.error:
    print('caught unbalanced paren')

# Pattern repr is stable
print(repr(re.compile(r'\d+')))

# --- fullmatch backtracks across alternation / lazy (#2626 review) ----------
print(re.fullmatch(r'a|ab', 'ab').group())
print(re.fullmatch(r'\d+|\d+x', '12x').group())
print(re.fullmatch(r'a.*?', 'abc').group())
print(re.fullmatch(r'(a)|(ab)', 'ab').groups())

# --- split emits the leading empty match (3.7+ / 3.12) ----------------------
print(re.split(r'x*', 'abc'))
print(re.split(r'', 'abc'))
print(re.split(r'a*', 'aXbXc'))
print(re.split(r'(x)*', 'abc'))

# --- \b inside a character class is backspace -------------------------------
print(bool(re.match(r'[\b]', '\b')))
print(bool(re.match(r'[^\b]', 'a')))
