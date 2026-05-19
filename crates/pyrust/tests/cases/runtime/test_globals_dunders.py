# Issue #675: globals() must include standard module-level dunder keys.
#
# CPython 3.12 always pre-populates __name__, __doc__, __package__, __spec__,
# __loader__, __annotations__, and __builtins__ in the module namespace before
# executing any user code.  Pyrust previously returned only user-defined names.
#
# The tests below check specific keys individually (not the whole dict) to
# avoid order-dependency.

# --- Presence checks ---

print("__name__ present:", "__name__" in globals())
print("__doc__ present:", "__doc__" in globals())
print("__annotations__ present:", "__annotations__" in globals())
print("__builtins__ present:", "__builtins__" in globals())
print("__spec__ present:", "__spec__" in globals())
print("__loader__ present:", "__loader__" in globals())
print("__package__ present:", "__package__" in globals())

# --- Value checks ---

print("__name__ == __main__:", globals()["__name__"] == "__main__")
print("__doc__ is None:", globals()["__doc__"] is None)
print("__annotations__ == {}:", globals()["__annotations__"] == {})
print("__spec__ is None:", globals()["__spec__"] is None)
print("__package__ is None:", globals()["__package__"] is None)

# __builtins__ is the builtins module (in __main__).
print("__builtins__ is module:", type(globals()["__builtins__"]).__name__ == "module")

# --- User assignments override the pre-seeded values ---

__name__ = "custom_name"
print("user override __name__:", globals()["__name__"] == "custom_name")

__doc__ = "user doc"
print("user override __doc__:", globals()["__doc__"] == "user doc")

# --- globals() from inside a function still returns the module namespace ---

def check_from_function():
    g = globals()
    print("__name__ in function globals:", "__name__" in g)
    print("function globals __name__:", g["__name__"])

check_from_function()
