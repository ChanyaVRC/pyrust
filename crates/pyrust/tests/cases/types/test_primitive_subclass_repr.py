# Regression test for issue #1600:
# Type.__repr__(subclass_instance) should use backing-data repr, not the
# generic `<module.ClassName object at 0x...>` form.  Triggered by PR #1595
# which set `base: Some(OBJECT_CLASS)` on primitive PyClass singletons,
# causing list.__repr__ to resolve via MRO to object.__repr__.

class MyList(list): pass
class MyDict(dict): pass
class MyTuple(tuple): pass
class MyBytes(bytes): pass
class MyStr(str): pass
# Use single-element sets to avoid non-deterministic ordering.
class MySet(set): pass
class MyFrozenSet(frozenset): pass

# repr() builtin — should still work (existing path, no regression).
print(repr(MyList([1, 2, 3])))
print(repr(MyDict({'a': 1})))
print(repr(MyTuple((1, 2))))
print(repr(MyBytes(b'hi')))
print(repr(MyStr('hello')))
print(repr(MySet({42})))
print(repr(MyFrozenSet({99})))

# Explicit Type.__repr__(instance) — previously broken.
print(list.__repr__(MyList([1, 2])))
print(dict.__repr__(MyDict({'x': 1})))
print(tuple.__repr__(MyTuple((3, 4))))
print(bytes.__repr__(MyBytes(b'world')))
print(str.__repr__(MyStr('test')))
print(set.__repr__(MySet({7})))
print(frozenset.__repr__(MyFrozenSet({8})))

# Subclass that defines its own __repr__: repr() should use it,
# but list.__repr__ should still render backing data.
class MyList2(list):
    def __repr__(self):
        return 'custom'

y = MyList2([10, 20])
print(repr(y))            # custom
print(list.__repr__(y))   # [10, 20]

# Empty containers.
print(repr(MyList([])))
print(repr(MySet()))
print(repr(MyFrozenSet()))
print(list.__repr__(MyList([])))
print(set.__repr__(MySet()))
print(frozenset.__repr__(MyFrozenSet()))
