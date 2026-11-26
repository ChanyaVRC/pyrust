# Issue #3293: pyrust builds the heavy `sys` attributes (`sys.monitoring`,
# `sys.stdlib_module_names`, `sys.flags`, the struct-sequence style values)
# on first use, and installs the `zipimport` path hook the first time a script
# can observe it.  That must be invisible: every lookup path, rebinding and
# deletion below behaves exactly as on CPython, where they exist from startup.

import sys

# --- The import system looks the way CPython bootstraps it -----------------
print("zipimport" in sys.modules, type(sys.path_hooks).__name__, len(sys.path_hooks) >= 1)
import zipimport

print(sys.path_hooks[0] is zipimport.zipimporter, sys.modules["zipimport"] is zipimport)
print(sys.path_hooks.count(zipimport.zipimporter))

# --- Deferred attributes are ordinary attributes ---------------------------
names = [
    "version_info",
    "flags",
    "float_info",
    "int_info",
    "hash_info",
    "implementation",
    "builtin_module_names",
    "stdlib_module_names",
    "thread_info",
    "monitoring",
]
print([hasattr(sys, name) for name in names])
print([name in dir(sys) for name in names])
print([name in vars(sys) for name in names])
print([getattr(sys, name) is getattr(sys, name) for name in names])
print([vars(sys)[name] is getattr(sys, name) for name in names])
print(vars(sys) is sys.__dict__, list(vars(sys))[:5])
print(any("." in name for name in dir(sys)))

print(type(sys.monitoring).__name__, sys.monitoring.__name__, sys.monitoring.DEBUGGER_ID)
print(len(sys.stdlib_module_names), type(sys.stdlib_module_names).__name__)
print("abc" in sys.stdlib_module_names, "zipimport" in sys.stdlib_module_names)
print(type(sys.flags).__name__, len(sys.flags), isinstance(sys.flags, tuple))
print(sys.flags.optimize, sys.flags.no_site, sys.flags.isolated, sys.flags.safe_path)
print(sys.float_info.max, sys.float_info.dig, sys.int_info.bits_per_digit)
print(sys.hash_info.width, sys.hash_info.modulus, sys.implementation.name)
print(isinstance(sys.thread_info, tuple), len(sys.thread_info))
print(sys.version_info[:2], sys.version_info >= (3, 12), sys.version_info.major)
print("sys" in sys.builtin_module_names, type(sys.builtin_module_names).__name__)
from sys import monitoring as imported_monitoring, flags as imported_flags

print(imported_monitoring is sys.monitoring, imported_flags is sys.flags)

# --- Deleting never resurrects, rebinding wins ------------------------------
del sys.monitoring
print(hasattr(sys, "monitoring"), "monitoring" in dir(sys), "monitoring" in vars(sys))
try:
    del sys.monitoring
except AttributeError:
    print("AttributeError")
try:
    sys.monitoring
except AttributeError:
    print("AttributeError")
print(hasattr(sys, "float_info"), hasattr(sys, "stdlib_module_names"))

sys.monitoring = "rebound"
print(sys.monitoring, list(vars(sys))[-1])

order_before = list(vars(sys))
sys.float_info = "replaced"
print(sys.float_info, list(vars(sys)) == order_before)
del sys.float_info
print(hasattr(sys, "float_info"), "float_info" in vars(sys))

sys.__dict__["hash_info"] = "via dict"
print(sys.hash_info)
del sys.__dict__["int_info"]
print(hasattr(sys, "int_info"))

# --- Rebinding the import state itself --------------------------------------
saved_hooks = sys.path_hooks
sys.path_hooks = []
print(sys.path_hooks, "zipimport" in sys.modules)
sys.path_hooks = saved_hooks
print(sys.path_hooks[0] is zipimport.zipimporter)
