import sys

# A fresh collections generation imports and captures itertools.chain.
for module_name in ("collections", "itertools"):
    if module_name in sys.modules:
        del sys.modules[module_name]
import collections
collections_imported_itertools = "itertools" in sys.modules
import itertools

old_chain = itertools.chain
old_iterator = old_chain.from_iterable([[1], [2]])


class OldChainSubclass(old_chain):
    pass


old_subclass_iterator = OldChainSubclass.from_iterable([[7]])

old_elements = collections.Counter(a=2).elements()

del sys.modules["itertools"]
import itertools as reloaded_itertools

new_chain = reloaded_itertools.chain
new_iterator = new_chain.from_iterable([[3], [4]])
old_factory_iterator = old_chain.from_iterable([[5]])
same_collections_elements = collections.Counter(b=2).elements()

print(
    "collections import:",
    collections_imported_itertools,
    type(old_elements) is old_chain,
)
print("classes:", old_chain is new_chain)
print(
    "old:",
    type(old_iterator) is old_chain,
    type(old_iterator) is new_chain,
    isinstance(old_iterator, old_chain),
    isinstance(old_iterator, new_chain),
)
print(
    "new:",
    type(new_iterator) is new_chain,
    type(new_iterator) is old_chain,
    isinstance(new_iterator, new_chain),
    isinstance(new_iterator, old_chain),
)
print(
    "old factory:",
    type(old_factory_iterator) is old_chain,
    type(old_factory_iterator) is new_chain,
)
print(
    "subclass:",
    type(old_subclass_iterator) is OldChainSubclass,
    isinstance(old_subclass_iterator, old_chain),
    isinstance(old_subclass_iterator, new_chain),
)
print(
    "old collections:",
    type(old_elements) is old_chain,
    type(old_elements) is new_chain,
    type(same_collections_elements) is old_chain,
    type(same_collections_elements) is new_chain,
)

# A fresh collections generation imports the current chain generation.
del sys.modules["collections"]
import collections as reloaded_collections

new_elements = reloaded_collections.Counter(c=2).elements()
print(
    "new collections:",
    type(new_elements) is old_chain,
    type(new_elements) is new_chain,
)
print(
    "values:",
    list(old_iterator),
    list(new_iterator),
    list(old_factory_iterator),
    list(old_subclass_iterator),
    list(old_elements),
    list(same_collections_elements),
    list(new_elements),
)
