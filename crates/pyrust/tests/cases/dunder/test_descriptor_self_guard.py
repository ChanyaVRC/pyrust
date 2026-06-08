# Issue #2026: the descriptor receiver-validation guard for type-qualified
# builtin methods (unbound descriptor calls such as `str.__len__()`) is now
# routed through the shared `descriptor_needs_arg!` / `descriptor_requires!`
# macros in pyrust-core.  This fixture pins the two CPython-3.12 message
# templates the macros produce so the consolidation can't drift:
#
#   no `self`  -> "descriptor '<m>' of '<type>' object needs an argument"
#   wrong type -> "descriptor '<m>' requires a '<type>' object but received a '<actual>'"
#
# Only cases whose pre-refactor wording already matches CPython 3.12 are
# asserted here; methods whose pyrust message historically diverges from
# CPython (e.g. the `unbound method ...` family) are deliberately excluded —
# fixing those is tracked separately (out of scope for the dedup).


def show(label, fn):
    try:
        fn()
    except TypeError as e:
        print(label, str(e))


# --- missing self argument: "... of '<type>' object needs an argument" -----
show("str.__len__", lambda: str.__len__())
show("list.__len__", lambda: list.__len__())
show("tuple.__len__", lambda: tuple.__len__())
show("dict.__len__", lambda: dict.__len__())
show("bytes.__len__", lambda: bytes.__len__())
show("object.__init__", lambda: object.__init__())


# --- wrong receiver type: "... requires a '<type>' object but received ..." -
show("str.__len__(int)", lambda: str.__len__(5))
show("list.__len__(int)", lambda: list.__len__(5))
show("tuple.__len__(int)", lambda: tuple.__len__(5))
show("dict.__len__(int)", lambda: dict.__len__(5))
show("bytes.__len__(int)", lambda: bytes.__len__(5))
show("str.__len__(list)", lambda: str.__len__([1, 2, 3]))
show("dict.__len__(str)", lambda: dict.__len__("ab"))
