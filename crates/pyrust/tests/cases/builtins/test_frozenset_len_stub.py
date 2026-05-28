# Issue #1548: frozenset.__len__ stub registration in helpers.rs
# Regression test: frozenset.__len__ should be callable directly as an
# unbound descriptor and should dispatch correctly for subclasses.

# Plain len() on a frozenset (regression guard)
print(len(frozenset({1, 2, 3})))

# Direct unbound call: frozenset.__len__(frozenset_instance)
print(frozenset.__len__(frozenset({1, 2, 3})))
print(frozenset.__len__(frozenset()))
print(frozenset.__len__(frozenset({42})))

# Subclass that overrides __len__ — must get the override, not the stub
class MyFS(frozenset):
    def __len__(self):
        return 99

print(len(MyFS({1, 2})))

# Subclass that does NOT override __len__ — must fall through to the stub
class BareFS(frozenset):
    pass

print(len(BareFS({10, 20, 30})))

# Error: wrong type passed to the descriptor
try:
    frozenset.__len__(42)
except TypeError as e:
    print(type(e).__name__)

# Error: no argument passed
try:
    frozenset.__len__()
except TypeError as e:
    print(type(e).__name__)
