# Parity fixture for str method dispatch: keyword argument handling (#1059)
# and TypeError vs RuntimeError for wrong argument types (#1060).

def check_type_error(desc, fn):
    try:
        fn()
        print(f"ERROR: {desc} should have raised TypeError")
    except TypeError as e:
        print(f"TypeError({desc}): {e}")

# ── Issue #1059: kwargs are passed through (not silently dropped) ─────────────

print(repr('hello world'.split(sep='x')))         # ['hello world']
print(repr('hello world'.split(maxsplit=0)))       # ['hello world']
print(repr('hello world'.split(maxsplit=1)))       # ['hello', 'world']
print(repr('hello world'.split(sep=' ', maxsplit=1)))  # ['hello', 'world']
print(repr('hello world'.rsplit(sep='x')))         # ['hello world']
print(repr('hello world'.rsplit(maxsplit=0)))      # ['hello world']
print(repr('hello world'.rsplit(maxsplit=1)))      # ['hello', 'world']
print(repr('a\nb'.splitlines(keepends=True)))      # ['a\n', 'b']
print(repr('a\nb'.splitlines(keepends=False)))     # ['a', 'b']
print(repr('a\tb'.expandtabs(tabsize=4)))          # 'a   b'
print(repr('a\tb'.expandtabs(tabsize=8)))          # 'a       b'

# ── Issue #1059: no-kwargs methods raise TypeError (with str. prefix) ─────────

check_type_error("upper(foo=)", lambda: 'hello'.upper(foo=1))
check_type_error("lower(foo=)", lambda: 'hello'.lower(foo=1))
check_type_error("strip(foo=)", lambda: 'hello'.strip(foo=1))
check_type_error("lstrip(foo=)", lambda: 'hello'.lstrip(foo=1))
check_type_error("rstrip(foo=)", lambda: 'hello'.rstrip(foo=1))
check_type_error("join(foo=)", lambda: 'x'.join(foo=1))
check_type_error("isalpha(foo=)", lambda: 'hello'.isalpha(foo=1))
check_type_error("isdigit(foo=)", lambda: 'hello'.isdigit(foo=1))
check_type_error("isspace(foo=)", lambda: 'hello'.isspace(foo=1))
check_type_error("find(foo=)", lambda: 'hello'.find(foo=1))
check_type_error("rfind(foo=)", lambda: 'hello'.rfind(foo=1))
check_type_error("index(foo=)", lambda: 'hello'.index(foo=1))
check_type_error("rindex(foo=)", lambda: 'hello'.rindex(foo=1))
check_type_error("count(foo=)", lambda: 'hello'.count(foo=1))
check_type_error("replace(count=kwarg)", lambda: 'hello'.replace('h', 'H', count=1))
check_type_error("startswith(foo=)", lambda: 'hello'.startswith(foo=1))
check_type_error("endswith(foo=)", lambda: 'hello'.endswith(foo=1))

# Unknown kwargs for split/rsplit/splitlines/expandtabs also raise TypeError
check_type_error("split(foo=)", lambda: 'hello'.split(foo='x'))
check_type_error("rsplit(foo=)", lambda: 'hello'.rsplit(foo='x'))
check_type_error("splitlines(foo=)", lambda: 'hello'.splitlines(foo=True))
check_type_error("expandtabs(foo=)", lambda: 'hello'.expandtabs(foo=4))

# Positional + keyword conflict for split/rsplit
check_type_error("split pos+kw conflict", lambda: 'hello world'.split('x', sep='y'))
check_type_error("rsplit pos+kw conflict", lambda: 'hello world'.rsplit('x', sep='y'))

# ── Issue #1060: wrong arg types raise TypeError (not RuntimeError) ───────────

check_type_error("find(int)", lambda: 'hello'.find(5))
check_type_error("rfind(int)", lambda: 'hello'.rfind(5))
check_type_error("index(int)", lambda: 'hello'.index(5))
check_type_error("rindex(int)", lambda: 'hello'.rindex(5))
check_type_error("count(int)", lambda: 'hello'.count(5))
check_type_error("split(int)", lambda: 'hello'.split(1))
check_type_error("rsplit(int)", lambda: 'hello'.rsplit(1))
check_type_error("replace arg1(int)", lambda: 'hello'.replace(1, 'x'))
check_type_error("replace arg2(int)", lambda: 'hello'.replace('h', 1))
check_type_error("startswith(int)", lambda: 'hello'.startswith(5))
check_type_error("endswith(int)", lambda: 'hello'.endswith(5))

# TypeError is catchable as TypeError
try:
    'hello'.find(5)
except TypeError:
    print("TypeError is catchable by except TypeError")
except Exception as e:
    print(f"ERROR: find(5) raised {type(e).__name__}, not TypeError")
