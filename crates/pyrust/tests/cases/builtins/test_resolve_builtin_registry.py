# Verify that every flat-namespace builtin is reachable by bare name (i.e.
# resolve_builtin works for all registry entries).  Issue #440: the old
# implementation maintained a 60-arm hardcoded match in parallel with the
# canonical REGISTRY; this fixture guards against future drift.

# --- callable built-in functions ---
print(callable(abs))           # True
print(callable(all))           # True
print(callable(any))           # True
print(callable(ascii))         # True
print(callable(bin))           # True
print(callable(callable))      # True
print(callable(chr))           # True
print(callable(classmethod))   # True
print(callable(delattr))       # True
print(callable(dir))           # True
print(callable(divmod))        # True
print(callable(enumerate))     # True
print(callable(filter))        # True
print(callable(format))        # True
print(callable(getattr))       # True
print(callable(globals))       # True
print(callable(hasattr))       # True
print(callable(hash))          # True
print(callable(hex))           # True
print(callable(id))            # True
print(callable(isinstance))    # True
print(callable(issubclass))    # True
print(callable(iter))          # True
print(callable(len))           # True
print(callable(locals))        # True
print(callable(map))           # True
print(callable(max))           # True
print(callable(min))           # True
print(callable(next))          # True
print(callable(oct))           # True
print(callable(open))          # True
print(callable(ord))           # True
print(callable(pow))           # True
print(callable(print))         # True
print(callable(property))      # True
print(callable(range))         # True
print(callable(repr))          # True
print(callable(reversed))      # True
print(callable(round))         # True
print(callable(setattr))       # True
print(callable(sorted))        # True
print(callable(staticmethod))  # True
print(callable(sum))           # True
print(callable(super))         # True
print(callable(type))          # True
print(callable(vars))          # True
print(callable(zip))           # True

# --- primitive type names resolve and are callable ---
print(callable(bool))          # True
print(callable(bytes))         # True
print(callable(complex))       # True
print(callable(dict))          # True
print(callable(float))         # True
print(callable(frozenset))     # True
print(callable(int))           # True
print(callable(list))          # True
print(callable(set))           # True
print(callable(str))           # True
print(callable(tuple))         # True

# --- NotImplemented is not callable but is accessible by bare name ---
print(NotImplemented)          # NotImplemented
print(callable(NotImplemented)) # False

# --- builtins actually work when called by bare name ---
print(abs(-5))                 # 5
print(len([1, 2, 3]))          # 3
print(min(3, 1, 2))            # 1
print(max(3, 1, 2))            # 3
print(sum([1, 2, 3]))          # 6
print(bin(10))                 # 0b1010
print(oct(8))                  # 0o10
print(hex(255))                # 0xff
print(chr(65))                 # A
print(ord('A'))                # 65
print(bool(0))                 # False
print(bool(1))                 # True
print(int("42"))               # 42
print(float("3.14"))           # 3.14
print(str(123))                # 123
print(repr("hello"))           # 'hello'
print(isinstance(42, int))     # True
print(isinstance("hi", str))   # True
print(issubclass(bool, int))   # True

# --- NameError for unknown names still works ---
try:
    _ = _nonexistent_builtin_xyz
except NameError as e:
    print(type(e).__name__)    # NameError
