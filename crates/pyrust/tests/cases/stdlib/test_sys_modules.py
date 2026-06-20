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
