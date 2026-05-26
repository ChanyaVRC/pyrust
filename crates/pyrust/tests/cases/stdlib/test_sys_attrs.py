# Parity fixture: sys.stdout, sys.stderr, sys.stdin, sys.path, sys.modules
# (issue #1127).
#
# NOTE: we do not test actual I/O to stderr here because the parity harness
# diffs stdout only; writing to stderr would not appear in the diff and could
# interfere with test output capture.  Instead we verify attribute existence,
# types, and method signatures.

import sys

# ── sys.stdout ───────────────────────────────────────────────────────────────

print(hasattr(sys, "stdout"))          # True
print(type(sys.stdout).__name__)       # TextIOWrapper
print(sys.stdout.name)                 # <stdout>
print(sys.stdout.mode)                 # w
print(sys.stdout.closed)               # False
print(sys.stdout.fileno())             # 1
result = sys.stdout.flush()
print(result)                          # None

# write() returns the number of characters written (not bytes).
n = sys.stdout.write("hi")
sys.stdout.write("\n")
print(n)                               # 2

# ── sys.stderr ───────────────────────────────────────────────────────────────

print(hasattr(sys, "stderr"))          # True
print(type(sys.stderr).__name__)       # TextIOWrapper
print(sys.stderr.name)                 # <stderr>
print(sys.stderr.mode)                 # w
print(sys.stderr.closed)               # False
print(sys.stderr.fileno())             # 2

# ── sys.stdin ────────────────────────────────────────────────────────────────

print(hasattr(sys, "stdin"))           # True
print(type(sys.stdin).__name__)        # TextIOWrapper
print(sys.stdin.name)                  # <stdin>
print(sys.stdin.mode)                  # r
print(sys.stdin.closed)                # False
print(sys.stdin.fileno())              # 0

# ── sys.path ─────────────────────────────────────────────────────────────────

print(hasattr(sys, "path"))            # True
print(isinstance(sys.path, list))      # True
print(len(sys.path) >= 1)             # True
# The first entry is a string (CPython: script dir; pyrust: "").
print(isinstance(sys.path[0], str))    # True

# sys.path is mutable.
sys.path.append("/fake/path")
print("/fake/path" in sys.path)        # True
sys.path.pop()

# ── sys.modules ──────────────────────────────────────────────────────────────

print(hasattr(sys, "modules"))         # True
print(isinstance(sys.modules, dict))   # True
