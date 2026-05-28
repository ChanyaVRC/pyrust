from __future__ import annotations


# Class-body annotations are stored as strings, not evaluated objects.
class C:
    x: int
    y: str


print(C.__annotations__)


# Function parameter and return annotations are also stored as strings.
def f(a: int, b: str) -> bool:
    pass


print(f.__annotations__)


# Forward reference no longer causes NameError at definition time.
class Node:
    next: Node


print(Node.__annotations__)


# More complex annotation expressions are preserved as source text.
class D:
    w: list[int]
    v: dict[str, int]
    t: int | str
    u: None
    e: ...


print(D.__annotations__)


# String-literal annotations retain their quotes (CPython 3.12 parity).
class E:
    z: 'Foo'


print(E.__annotations__)


# Forward references in function annotations work at definition time.
def process(node: TreeNode) -> TreeNode:
    return node


class TreeNode:
    def __init__(self, val: int):
        self.val = val


print(process(TreeNode(5)).val)


# Nested class annotations also use lazy evaluation.
class Outer:
    class Inner:
        ref: Outer


print(Outer.Inner.__annotations__)
