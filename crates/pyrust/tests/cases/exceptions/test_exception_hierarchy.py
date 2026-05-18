# Parity fixture for issue #574: intermediate exception base classes and correct
# hierarchy wiring.  Checks that:
#   * ArithmeticError, LookupError, BaseException, SyntaxError, ImportError,
#     FloatingPointError, ModuleNotFoundError are all accessible by name.
#   * except ArithmeticError catches OverflowError and ZeroDivisionError but
#     not ValueError.
#   * except LookupError catches IndexError and KeyError but not TypeError.
#   * issubclass reflects the correct hierarchy.
#   * except Exception does NOT catch SystemExit or GeneratorExit (they derive
#     from BaseException, not Exception).

# --- Name accessibility ---
print(BaseException.__name__)
print(Exception.__name__)
print(ArithmeticError.__name__)
print(LookupError.__name__)
print(FloatingPointError.__name__)
print(SyntaxError.__name__)
print(ImportError.__name__)
print(ModuleNotFoundError.__name__)

# --- ArithmeticError catches its leaf subclasses ---
try:
    1 / 0
except ArithmeticError:
    print("ZeroDivisionError caught by ArithmeticError")

try:
    raise OverflowError("overflow")
except ArithmeticError:
    print("OverflowError caught by ArithmeticError")

try:
    raise FloatingPointError("fp")
except ArithmeticError:
    print("FloatingPointError caught by ArithmeticError")

# ArithmeticError must NOT catch unrelated exceptions
try:
    raise ValueError("not arithmetic")
except ArithmeticError:
    print("WRONG: ValueError caught by ArithmeticError")
except ValueError:
    print("ValueError not caught by ArithmeticError")

# --- LookupError catches its leaf subclasses ---
try:
    _ = [][0]
except LookupError:
    print("IndexError caught by LookupError")

try:
    _ = {}["missing"]
except LookupError:
    print("KeyError caught by LookupError")

# LookupError must NOT catch TypeError
try:
    raise TypeError("not lookup")
except LookupError:
    print("WRONG: TypeError caught by LookupError")
except TypeError:
    print("TypeError not caught by LookupError")

# --- issubclass checks ---
print(issubclass(OverflowError, ArithmeticError))
print(issubclass(ZeroDivisionError, ArithmeticError))
print(issubclass(FloatingPointError, ArithmeticError))
print(issubclass(ArithmeticError, Exception))
print(issubclass(IndexError, LookupError))
print(issubclass(KeyError, LookupError))
print(issubclass(LookupError, Exception))
print(issubclass(Exception, BaseException))
print(issubclass(SystemExit, BaseException))
print(issubclass(GeneratorExit, BaseException))
print(issubclass(FileNotFoundError, OSError))
print(issubclass(ModuleNotFoundError, ImportError))

# SystemExit must NOT be caught by except Exception
print(issubclass(SystemExit, Exception))
print(issubclass(GeneratorExit, Exception))

# --- except Exception does NOT catch SystemExit ---
try:
    raise SystemExit(0)
except Exception:
    print("WRONG: SystemExit caught by Exception")
except SystemExit:
    print("SystemExit not caught by Exception")

# --- except Exception does NOT catch GeneratorExit ---
try:
    raise GeneratorExit()
except Exception:
    print("WRONG: GeneratorExit caught by Exception")
except GeneratorExit:
    print("GeneratorExit not caught by Exception")
