# Issue #2727: sys.modules is populated during imports and is a stable shared
# dict that acts as the import cache.  Assert membership of specific imported
# names and identity/cache behaviour rather than an exact len() (which varies
# across CPython patch releases).
import os
import sys

# Imported modules appear in sys.modules.
print("os" in sys.modules)
print("sys" in sys.modules)
print("builtins" in sys.modules)
print(len(sys.modules) > 0)

# sys.modules is a real dict.
print(type(sys.modules).__name__)

# sys.modules returns the same underlying dict on repeated access — mutations
# are stable and observable across accesses.
sys.modules["sentinel_2727"] = 123
print("sentinel_2727" in sys.modules)
print(sys.modules["sentinel_2727"])
del sys.modules["sentinel_2727"]
print("sentinel_2727" in sys.modules)

# The cached object is identical to the imported name.
print(sys.modules["os"] is os)
print(sys.modules["sys"] is sys)

# Re-importing an already-imported module returns the cached object (no
# re-execution); the bound name is the same object.
import os as os_again

print(os_again is os)

# Importing a submodule registers it under its dotted name.
import os.path

print("os.path" in sys.modules)
print(sys.modules["os.path"] is os.path)

# sys.modules is the authoritative cache: a value injected directly by user
# code makes the matching `import` a cache hit that returns the injected object
# (issue #2727 review).
class _FakeModule:
    pass


_fake = _FakeModule()
sys.modules["fake_injected_2727"] = _fake
import fake_injected_2727

print(fake_injected_2727 is _fake)

# Rebinding the attribute replaces the authoritative import registry.  Imports
# must populate the replacement dict rather than an Interpreter-private stale
# handle.
_original_modules = sys.modules
_replacement_modules = {}
sys.modules = _replacement_modules
import math as _math_after_registry_replacement

print(sys.modules is _replacement_modules)
print(_replacement_modules["math"] is _math_after_registry_replacement)

# CPython permits assigning a non-dict; the failure occurs when importlib next
# asks the replacement for its dict-like `get` operation.
sys.modules = 1
try:
    import math as _math_with_invalid_registry
except Exception as _invalid_registry_error:
    print(type(_invalid_registry_error).__name__)
    print(str(_invalid_registry_error))

# Deleting the attribute likewise succeeds, but the next import observes that
# the canonical sys module no longer exposes the registry.
sys.modules = _replacement_modules
del sys.modules
try:
    import math as _math_without_registry
except Exception as _missing_registry_error:
    print(type(_missing_registry_error).__name__)
    print(str(_missing_registry_error))

# Leave the process in a usable state for interpreter shutdown and for runners
# that execute more than one fixture in the same process.
sys.modules = _original_modules
