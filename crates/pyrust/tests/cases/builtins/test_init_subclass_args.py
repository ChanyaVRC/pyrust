# Parity fixture for issue #1739: object.__init_subclass__() must raise
# TypeError when called with excess positional or keyword arguments.

# --- No arguments: should succeed ---
try:
    object.__init_subclass__()
    print("no args: ok")
except TypeError as e:
    print(f"no args: unexpected TypeError: {e}")

# --- One positional arg (excess): should raise TypeError ---
try:
    object.__init_subclass__(42)
    print("one positional: no error (wrong)")
except TypeError as e:
    print(f"one positional: TypeError: {e}")

# --- Two positional args (excess): should raise TypeError with count 2 ---
try:
    object.__init_subclass__(42, 43)
    print("two positionals: no error (wrong)")
except TypeError as e:
    print(f"two positionals: TypeError: {e}")

# --- Keyword argument: should raise TypeError ---
try:
    object.__init_subclass__(x=1)
    print("keyword: no error (wrong)")
except TypeError as e:
    print(f"keyword: TypeError: {e}")

# --- Both positional and keyword: keyword error takes precedence ---
try:
    object.__init_subclass__(42, x=1)
    print("positional+keyword: no error (wrong)")
except TypeError as e:
    print(f"positional+keyword: TypeError: {e}")

# --- Normal class creation (no kwargs) works ---
class A:
    pass

print("class A (no kwargs): ok")

# --- Class with kwargs forwarded to object.__init_subclass__ raises TypeError ---
try:
    class B(metaclass=type, bad_kwarg=1):
        pass
    print("class B with bad_kwarg: no error (wrong)")
except TypeError as e:
    print(f"class B with bad_kwarg: TypeError: {e}")

# --- Subclass positional error uses the subclass name, not "object" ---
class C:
    pass

try:
    C.__init_subclass__(42)
    print("C one positional: no error (wrong)")
except TypeError as e:
    print(f"C one positional: TypeError: {e}")
