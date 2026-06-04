# Issue #2122: set / frozenset / dict operator and in-place algebra/merge
# dunders are exposed as bound method-wrappers, dispatching through the same
# machinery as the |, &, -, ^, |= operators (set algebra + PEP 584 dict merge).


def show(expr, fn):
    try:
        print(repr(fn()))
    except Exception as ex:
        print(expr, "->", type(ex).__name__, ex)


# --- set forward algebra ----------------------------------------------------
print(sorted({1, 2}.__or__({3})))
print(sorted({1, 2}.__and__({2, 3})))
print(sorted({1, 2}.__sub__({2})))
print(sorted({1, 2}.__xor__({2, 3})))

# --- set reflected ----------------------------------------------------------
print(sorted({1, 2}.__ror__({3})))
print(sorted({1, 2}.__rand__({2, 3})))
print(sorted({1, 2}.__rsub__({1, 2, 3})))
print(sorted({1, 2}.__rxor__({2, 3})))

# --- set in-place (mutate + return self) ------------------------------------
s = {1, 2}
print(s.__ior__({3}) is s, sorted(s))
s2 = {1, 2, 3}
print(sorted(s2.__iand__({2, 3})))
s3 = {1, 2, 3}
print(sorted(s3.__isub__({2})))
s4 = {1, 2, 3}
print(sorted(s4.__ixor__({3, 4})))

# --- frozenset: forward + reflected, no in-place ----------------------------
print(sorted(frozenset({1}).__or__({2})))
print(type(frozenset({1}).__or__({2})).__name__)
print(sorted(frozenset({1, 2}).__and__({2, 3})))
print(hasattr(frozenset(), "__ior__"), hasattr(frozenset(), "__iand__"))

# --- dict PEP 584 merge -----------------------------------------------------
print({1: 1}.__or__({2: 2}))
print({1: 1}.__ror__({2: 2}))
d = {1: 1}
print(d.__ior__({2: 2}) is d, d)
d2 = {1: 1}
print(d2.__ior__([(3, 3)]))

# --- NotImplemented for incompatible operands (not TypeError) ---------------
print(repr({1}.__or__([2])))
print(repr({1}.__ior__([2])))
print(repr(frozenset({1}).__or__([2])))
print(repr({1: 1}.__or__([2])))
print(repr({1: 1}.__ror__([2])))

# --- hasattr / dir consistency ----------------------------------------------
print(hasattr({1}, "__or__"), hasattr({1}, "__isub__"))
print(hasattr({}, "__or__"), hasattr(frozenset(), "__sub__"))
print(hasattr({}, "__ror__"), hasattr({}, "__ior__"))
print("__or__" in dir(set), "__ior__" in dir(set), "__ior__" in dir(frozenset))
print("__or__" in dir(dict), "__ror__" in dir(dict), "__ior__" in dir(dict))

# --- unbound type-level form ------------------------------------------------
print(set.__or__({1}, {2}) == {1, 2})
print(dict.__or__({1: 1}, {2: 2}))
print(frozenset.__sub__(frozenset({1, 2}), {1}) == frozenset({2}))

# --- arity errors -----------------------------------------------------------
show("{1}.__or__()", lambda: {1}.__or__())
show("{1}.__ior__()", lambda: {1}.__ior__())
show("{1:1}.__or__()", lambda: {1: 1}.__or__())
