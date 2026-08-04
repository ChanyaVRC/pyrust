# Issue #3027: class-frame f_locals is the live class namespace.  Both the
# class body itself and a helper looking at its caller must receive the same
# mapping, and mutations through that mapping must survive class finalization.
import sys


def write_outer_namespace():
    namespace = sys._getframe(1).f_locals
    namespace["known"] = "outer-known"
    namespace["outer_only"] = "outer-added"
    return namespace is sys._getframe(1).f_locals


class OuterWrite:
    known = "before"
    same_mapping = write_outer_namespace()
    known_read = known
    outer_only_read = outer_only


print("outer identity:", OuterWrite.same_mapping)
print("outer known:", OuterWrite.known)
print("outer only:", OuterWrite.outer_only)
print("outer bare reads:", OuterWrite.known_read, OuterWrite.outer_only_read)


class OwnWrite:
    known = "before"
    namespace = sys._getframe().f_locals
    namespace["known"] = "own-known"
    namespace["own_only"] = "own-added"
    same_mapping = (
        namespace is sys._getframe().f_locals
        and namespace is locals()
        and namespace is vars()
    )
    known_read = known
    own_only_read = own_only


print("own identity:", OwnWrite.same_mapping)
print("own known:", OwnWrite.known)
print("own only:", OwnWrite.own_only)
print("own bare reads:", OwnWrite.known_read, OwnWrite.own_only_read)


deleted_fallback = "module"


class OwnDelete:
    kept = "kept"
    doomed = "present"
    deleted_fallback = "class"
    rebound = "before"
    seed_prefix = list(sys._getframe().f_locals)[:2]
    del sys._getframe().f_locals["doomed"]
    del sys._getframe().f_locals["deleted_fallback"]
    fallback_read = deleted_fallback
    del sys._getframe().f_locals["rebound"]
    sys._getframe().f_locals["overlay_only"] = "overlay"
    rebound = "after"


print("seed prefix:", OwnDelete.seed_prefix)
print("own delete:", "doomed" in vars(OwnDelete))
print("deleted bare read:", OwnDelete.fallback_read)
print("rebound:", OwnDelete.rebound)
print(
    "final order:",
    [
        name
        for name in vars(OwnDelete)
        if name in ("kept", "doomed", "seed_prefix", "overlay_only", "rebound")
    ],
)


class AnnotationSync:
    namespace = sys._getframe().f_locals
    before: int = 4
    namespace["__annotations__"] = {"mapped": str}
    after: float = 5


print(
    "annotation mapping:",
    list(AnnotationSync.__annotations__),
    AnnotationSync.__annotations__["mapped"] is str,
    AnnotationSync.__annotations__["after"] is float,
)


delete_fallback = "module"


class BareDeleteOverlay:
    namespace = sys._getframe().f_locals
    namespace["delete_fallback"] = "mapped"
    del delete_fallback
    seen = delete_fallback


print(
    "bare delete overlay:",
    hasattr(BareDeleteOverlay, "delete_fallback"),
    BareDeleteOverlay.seen,
)


class BareDeleteMissing:
    known = "class"
    namespace = sys._getframe().f_locals
    del namespace["known"]
    try:
        del known
    except NameError:
        missing_raises = True


print(
    "bare delete missing:",
    hasattr(BareDeleteMissing, "known"),
    BareDeleteMissing.missing_raises,
)


class WalrusStore:
    namespace = sys._getframe().f_locals
    (walrus := "stored")


print(
    "walrus store:",
    WalrusStore.namespace["walrus"],
    WalrusStore.walrus,
)


explicit_global = "module"


class ExplicitGlobal:
    global explicit_global

    namespace = sys._getframe().f_locals
    namespace["explicit_global"] = "overlay"
    seen = explicit_global


print(
    "explicit global:",
    ExplicitGlobal.explicit_global,
    ExplicitGlobal.seen,
    explicit_global,
)


class SeedRebindOrder:
    del __qualname__
    marker = "middle"
    __qualname__ = "Rebound"
    order = [
        name
        for name in sys._getframe().f_locals
        if name in ("__module__", "marker", "__qualname__")
    ]


print("seed rebind order:", SeedRebindOrder.order, SeedRebindOrder.__qualname__)


def make_class_reader():
    captured = "enclosing"

    def reader():
        class CapturedRead:
            namespace = sys._getframe().f_locals
            seen = captured

        return CapturedRead

    return reader


class_reader = make_class_reader()
print(
    "class fallback closure:",
    class_reader.__code__.co_freevars,
    class_reader().seen,
)


def make_class_local_reader():
    class_local = "grandparent"

    def reader():
        class LocalRead:
            class_local = "local"
            del class_local
            seen = class_local

        return LocalRead

    return reader


class_local_reader = make_class_local_reader()
class_local_raises = False
try:
    class_local_reader()
except NameError:
    class_local_raises = True

print(
    "class local closure:",
    class_local_reader.__code__.co_freevars,
    class_local_raises,
)
