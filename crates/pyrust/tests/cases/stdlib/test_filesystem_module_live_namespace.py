import sys

import _filesystem_namespace_owner as module


# Module attributes and function globals are one namespace in both directions.
module.x = 2
print("attribute-to-function", module.x, module.read())
module.write(3)
print("function-to-attribute", module.x, module.read())

# Every Python-visible namespace accessor returns the same live dict.
namespace = module.__dict__
print(
    "identity",
    namespace is module.__dict__,
    vars(module) is namespace,
    module.read.__globals__ is namespace,
)
namespace["x"] = 4
print("dict-to-both", module.x, module.read())

# import-from observes the same current binding.
from _filesystem_namespace_owner import x as imported_x

print("import-from", imported_x)

# import-star reads the same live backing, both with an explicit __all__ and
# with the default public-name filtering.
from _filesystem_namespace_owner import *

print("import-star-all", x, read())

import _filesystem_namespace_no_all as no_all

no_all.__dict__["dynamic"] = 9
from _filesystem_namespace_no_all import *

print("import-star-default", public, dynamic, "_hidden" in globals())

# Deletion propagates in both directions and a later dict write revives the
# global for both module and function access.
del module.x
print("attribute-del", "x" in namespace, hasattr(module, "x"))
try:
    module.read()
except NameError:
    print("attribute-del-function", "NameError")

namespace["x"] = 5
print("dict-revive", module.x, module.read())
module.remove()
print("function-del", "x" in namespace, hasattr(module, "x"))

# A circular import sees the already-executed portion through the same live
# module namespace rather than an empty post-execution snapshot.
import _filesystem_namespace_cycle_a as cycle

print("circular", cycle.seen_by_peer)
print("circular-reverse-write", cycle.seen_after_peer_write, cycle.value)

# Failed imports preserve the original typed exception and are removed from
# sys.modules so a second import retries rather than returning a partial module.
for attempt in (1, 2):
    try:
        import _filesystem_namespace_failed
    except RuntimeError as exc:
        print(
            "failed",
            attempt,
            type(exc).__name__,
            str(exc),
            "_filesystem_namespace_failed" in sys.modules,
        )
        inner = exc.__traceback__
        while inner.tb_next is not None:
            inner = inner.tb_next
        print(
            "failed-frame",
            inner.tb_frame.f_globals["error"] is exc,
            inner.tb_frame.f_globals["partial"],
        )
