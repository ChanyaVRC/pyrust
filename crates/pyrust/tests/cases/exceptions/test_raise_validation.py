# Parity fixture for issue #1083: raise <non-exception> must raise TypeError
# (not RuntimeError), and `raise X from Y` must validate the cause.

# --- raise <non-exception> ---

try:
    raise 42
except TypeError as e:
    print("raise int -> TypeError:", e)

try:
    raise "oops"
except TypeError as e:
    print("raise str -> TypeError:", e)

try:
    raise 3.14
except TypeError as e:
    print("raise float -> TypeError:", e)

# Non-exception class raises TypeError.
class NotAnException:
    pass

try:
    raise NotAnException()
except TypeError as e:
    print("raise non-exc instance -> TypeError:", e)

try:
    raise NotAnException
except TypeError as e:
    print("raise non-exc class -> TypeError:", e)

# --- raise ExceptionClass (bare class, auto-instantiated) ---

try:
    raise ValueError
except ValueError as e:
    print("raise class -> ValueError, args =", repr(e.args))

# --- raise X from <non-exception> ---

try:
    raise ValueError("v") from 42
except TypeError as e:
    print("raise from int -> TypeError:", e)

try:
    raise ValueError("v") from "bad"
except TypeError as e:
    print("raise from str -> TypeError:", e)

try:
    raise ValueError("v") from NotAnException()
except TypeError as e:
    print("raise from non-exc instance -> TypeError:", e)

try:
    raise ValueError("v") from NotAnException
except TypeError as e:
    print("raise from non-exc class -> TypeError:", e)

# --- raise X from None (valid, sets __cause__=None, __suppress_context__=True) ---

try:
    raise ValueError("v") from None
except ValueError as e:
    print("raise from None -> cause =", repr(e.__cause__), "suppress =", e.__suppress_context__)

# --- raise X from ExceptionInstance (valid) ---

try:
    raise ValueError("v") from TypeError("t")
except ValueError as e:
    print("raise from exc instance -> cause =", repr(e.__cause__))

# --- raise X from ExceptionClass (valid, cause auto-instantiated) ---

try:
    raise ValueError("v") from TypeError
except ValueError as e:
    print("raise from exc class -> cause type =", type(e.__cause__).__name__)
