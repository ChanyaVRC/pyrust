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

# None is importlib's explicit negative-cache sentinel, not a successful module
# value. It must win even when an internal cache still owns a canonical module.
sys.modules["blocked_by_none_2727"] = None
try:
    import blocked_by_none_2727
except Exception as _none_sentinel_error:
    print(type(_none_sentinel_error).__name__)
    print(str(_none_sentinel_error))
del sys.modules["blocked_by_none_2727"]

# Rebinding the attribute replaces the importlib-visible registry for names not
# already present in the interpreter's original dict. Existing internal entries
# retain priority; newly loaded names must populate the replacement.
_original_modules = sys.modules
_replacement_modules = {"os": _fake}
sys.modules = _replacement_modules
import os as _os_after_registry_replacement

print(_os_after_registry_replacement is os)
print(_replacement_modules["os"] is _fake)
import math as _math_after_registry_replacement

print(sys.modules is _replacement_modules)
print(_replacement_modules["math"] is _math_after_registry_replacement)

# CPython permits assigning a non-dict; the failure occurs when importlib next
# asks the replacement for its dict-like `get` operation. An existing internal
# entry does not consult the invalid replacement at all.
sys.modules = 1
import os as _os_with_invalid_registry

print(_os_with_invalid_registry is os)
try:
    import math as _math_with_invalid_registry
except Exception as _invalid_registry_error:
    print(type(_invalid_registry_error).__name__)
    print(str(_invalid_registry_error))

# Deleting the attribute likewise succeeds, but the next import observes that
# the canonical sys module no longer exposes the registry. Existing internal
# entries remain importable without that attribute.
sys.modules = _replacement_modules
del sys.modules
import os as _os_without_registry

print(_os_without_registry is os)
try:
    import math as _math_without_registry
except Exception as _missing_registry_error:
    print(type(_missing_registry_error).__name__)
    print(str(_missing_registry_error))

# Leave the process in a usable state for interpreter shutdown and for runners
# that execute more than one fixture in the same process.
sys.modules = _original_modules

# A module's __dict__ and vars(module) expose the exact writable namespace.
# Import-registry caching must observe rebinding through either alias.
_sys_namespace = sys.__dict__
print(vars(sys) is _sys_namespace)
_dict_alias_registry = {}
_sys_namespace["modules"] = _dict_alias_registry
import math as _math_from_dict_alias

print(sys.modules is _dict_alias_registry)
print(_dict_alias_registry["math"] is _math_from_dict_alias)
_sys_namespace["modules"] = _original_modules

# A real dict subclass remains a valid registry. Its observable mapping
# overrides must run rather than being bypassed through primitive backing.
class _Registry(dict):
    def get(self, name, default=None):
        self.get_seen = name == "virtual_registry_2727" or self.get_seen
        if name == "virtual_registry_2727":
            return 42
        return dict.get(self, name, default)

    def __setitem__(self, name, value):
        self.set_seen = name == "math" or self.set_seen
        return dict.__setitem__(self, name, value)

    def __delitem__(self, name):
        self.delete_seen = name == "_registry_delete_failure" or self.delete_seen
        return dict.__delitem__(self, name)


_subclass_registry = _Registry(_original_modules)
_subclass_registry.get_seen = False
_subclass_registry.set_seen = False
_subclass_registry.delete_seen = False
_subclass_registry.pop("math", None)
sys.modules = _subclass_registry
import virtual_registry_2727
import math as _math_from_subclass

print(virtual_registry_2727 == 42)
print(_subclass_registry.get_seen)
print(_subclass_registry["math"] is _math_from_subclass)
print(_subclass_registry.set_seen)
try:
    import _registry_delete_failure
except RuntimeError:
    pass
print(_subclass_registry.delete_seen)
print("_registry_delete_failure" not in _subclass_registry)
sys.modules = _original_modules

# `sys` itself is interpreter state. Removing it from a replacement registry
# must not manufacture a second module object on re-import.
_canonical_sys = sys
_sysless_registry = {}
sys.modules = _sysless_registry
import sys as _reimported_sys

print(_reimported_sys is _canonical_sys)
print("sys" not in _sysless_registry)
_canonical_sys.modules = _original_modules

# The original dictionary is the interpreter cache itself. Deleting its `sys`
# entry is therefore different from merely replacing the public attribute:
# CPython creates and registers a new import identity.
del _original_modules["sys"]
import sys as _fresh_sys

print(_fresh_sys is _canonical_sys)
print(_original_modules["sys"] is _fresh_sys)
_original_modules["sys"] = _canonical_sys
