import builtins
import itertools
import math
import sys


# A native callable belongs to one concrete built-in module generation.
old_math = math
old_sqrt = math.sqrt
print(old_sqrt is math.sqrt, old_sqrt == math.sqrt)
print(old_sqrt.__module__)
old_sqrt.__module__ = "changed"
print(old_sqrt.__module__, math.sqrt.__module__)

del sys.modules["math"]
import math as new_math

print(old_math is new_math)
print(old_sqrt is new_math.sqrt, old_sqrt == new_math.sqrt)
print(hash(old_sqrt) == hash(new_math.sqrt))
print(id(old_sqrt) == id(new_math.sqrt))
print(len({old_sqrt, new_math.sqrt}))
print(old_sqrt.__module__, new_math.sqrt.__module__)
del old_sqrt.__module__
print(old_sqrt.__module__, new_math.sqrt.__module__)


# Flat builtins retain one shared object identity, including across rebuilding
# the `builtins` module, and carry one shared mutable __module__ slot.
old_builtins = builtins
print(len is builtins.len)
len.__module__ = "flat-changed"
print(len.__module__, builtins.len.__module__)

del sys.modules["builtins"]
import builtins as new_builtins

print(old_builtins is new_builtins)
print(len is new_builtins.len, len == new_builtins.len)
print(len.__module__, new_builtins.len.__module__)
del len.__module__
print(len.__module__, new_builtins.len.__module__)


# A captured built-in method owns its __module__ slot independently from a
# second capture of the same receiver/method pair.
items = []
first_append = items.append
second_append = items.append
print(
    first_append is second_append,
    first_append == second_append,
    hash(first_append) == hash(second_append),
)
print(first_append.__module__, second_append.__module__)
first_append.__module__ = "captured"
print(
    first_append == second_append,
    hash(first_append) == hash(second_append),
    first_append.__module__,
    second_append.__module__,
)
del first_append.__module__
print(first_append.__module__, second_append.__module__)
first_len = items.__len__
second_len = items.__len__
print(
    first_len is second_len,
    first_len == second_len,
    hash(first_len) == hash(second_len),
)
print((1).bit_length == True.bit_length)
first_append(42)
print(items)


# Native class/static wrappers share the same mutable category boundary. A
# class-bound wrapper is recaptured per access; a static wrapper is stable.
first_fromkeys = dict.fromkeys
second_fromkeys = dict.fromkeys
print(
    first_fromkeys is second_fromkeys,
    first_fromkeys == second_fromkeys,
    hash(first_fromkeys) == hash(second_fromkeys),
    first_fromkeys.__module__,
    second_fromkeys.__module__,
)
first_fromkeys.__module__ = "native-class"
print(
    first_fromkeys == second_fromkeys,
    hash(first_fromkeys) == hash(second_fromkeys),
    first_fromkeys.__module__,
    second_fromkeys.__module__,
)
del first_fromkeys.__module__
print(first_fromkeys.__module__, second_fromkeys.__module__)
print(first_fromkeys("ab", 1))

first_maketrans = bytes.maketrans
second_maketrans = bytes.maketrans
print(first_maketrans is second_maketrans)
first_maketrans.__module__ = "native-static"
print(first_maketrans.__module__, second_maketrans.__module__)
del first_maketrans.__module__
print(first_maketrans.__module__, second_maketrans.__module__)
print(first_maketrans(b"a", b"b")[97])


# Native classes and their descriptors are generation-local too.
old_itertools = itertools
old_chain = itertools.chain
old_chain_next = itertools.chain.__next__

del sys.modules["itertools"]
import itertools as new_itertools

print(old_itertools is new_itertools)
print(old_chain is new_itertools.chain)
print(old_chain_next is new_itertools.chain.__next__)
print(old_chain_next == new_itertools.chain.__next__)
print(hash(old_chain_next) == hash(new_itertools.chain.__next__))
print(id(old_chain_next) == id(new_itertools.chain.__next__))
