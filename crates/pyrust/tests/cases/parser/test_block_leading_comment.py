# Parity fixture for issue #494: leading comment-only line at the top of any
# suite-introducing block.  CPython accepts comments at any position; pyrust
# was previously rejecting them when they appeared as the very first line of
# a block (before the first real statement).
#
# Covers: if / else / elif / while / for / try / except / finally / with /
#         def / class / match, plus multi-comment and blank+comment mixes.

# ── if ──────────────────────────────────────────────────────────────────────
if True:
    # leading comment in if body
    print("if")

# ── else ────────────────────────────────────────────────────────────────────
if False:
    pass
else:
    # leading comment in else body
    print("else")

# ── elif ────────────────────────────────────────────────────────────────────
x = 1
if x == 0:
    pass
elif x == 1:
    # leading comment in elif body
    print("elif")

# ── while ───────────────────────────────────────────────────────────────────
_n = 0
while _n < 1:
    # leading comment in while body
    print("while")
    _n += 1

# ── for ─────────────────────────────────────────────────────────────────────
for _ in range(1):
    # leading comment in for body
    print("for")

# ── try body ────────────────────────────────────────────────────────────────
try:
    # leading comment in try body
    pass
except Exception:
    pass

# ── except handler ──────────────────────────────────────────────────────────
try:
    int("bad")
except ValueError as e:
    # leading comment in except handler (the original repro from issue #494)
    print("except", type(e).__name__)

# ── finally ─────────────────────────────────────────────────────────────────
try:
    pass
finally:
    # leading comment in finally body
    print("finally")

# ── try/else ────────────────────────────────────────────────────────────────
try:
    pass
except Exception:
    pass
else:
    # leading comment in try-else body
    print("try-else")

# ── with ────────────────────────────────────────────────────────────────────
class _CM:
    def __enter__(self): return self
    def __exit__(self, *a): pass

with _CM():
    # leading comment in with body
    print("with")

# ── def ─────────────────────────────────────────────────────────────────────
def _func_comment():
    # leading comment in function body
    return "def"

print(_func_comment())

# ── class ───────────────────────────────────────────────────────────────────
class _ClassComment:
    # leading comment in class body
    val = "class"

print(_ClassComment.val)

# ── match ───────────────────────────────────────────────────────────────────
_v = 42
match _v:
    # leading comment before first case arm
    case 42:
        # leading comment in case body
        print("match")
    case _:
        pass

# ── multiple comment lines ───────────────────────────────────────────────────
def _multi():
    # first comment
    # second comment
    # third comment
    return "multi"

print(_multi())

# ── blank line then comment line ─────────────────────────────────────────────
def _blank_then_comment():

    # comment after blank line
    return "blank-then-comment"

print(_blank_then_comment())

# ── comment then blank then code ─────────────────────────────────────────────
def _comment_blank_code():
    # comment

    return "comment-blank-code"

print(_comment_blank_code())

# ── nested: comment in outer and inner ───────────────────────────────────────
def _outer():
    # leading comment in outer
    def _inner():
        # leading comment in inner
        return "nested"
    return _inner()

print(_outer())
