# Parity fixture: SystemExit.code attribute — CPython 3.12 semantics.
# 0 args -> None, 1 arg -> the arg, 2+ args -> tuple of all args.

try:
    raise SystemExit(42)
except SystemExit as e:
    print(e.code)          # 42

try:
    raise SystemExit("abort")
except SystemExit as e:
    print(e.code)          # abort

try:
    raise SystemExit()
except SystemExit as e:
    print(e.code)          # None

try:
    raise SystemExit(1, 2)
except SystemExit as e:
    print(e.code)          # (1, 2)

# Subclass inherits the .code population
class MyExit(SystemExit):
    pass

try:
    raise MyExit(99)
except SystemExit as e:
    print(e.code)          # 99
