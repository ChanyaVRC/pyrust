# A subclass of a non-iterable builtin is itself not iterable.  The
# TypeError must name the actual subclass (type(obj).__name__), not the
# builtin base it inherits from (#2557).


class C(int):
    pass


class F(float):
    pass


class X(complex):
    pass


# iter() builtin
try:
    iter(C(5))
except TypeError as e:
    print("iter:", e)

# list.extend
try:
    list.extend([], C(5))
except TypeError as e:
    print("extend:", e)

# for-loop iteration
try:
    for _ in C(5):
        pass
except TypeError as e:
    print("for:", e)

# other constructors / consumers that materialize an iterable
try:
    list(F(1.0))
except TypeError as e:
    print("list:", e)

try:
    tuple(X(1))
except TypeError as e:
    print("tuple:", e)

try:
    set(C(7))
except TypeError as e:
    print("set:", e)

try:
    sum(C(3))
except TypeError as e:
    print("sum:", e)

# Iterable builtin subclasses keep iterating their backing primitive.
class L(list):
    pass


class S(str):
    pass


print("list-subclass:", list(L([1, 2, 3])))
print("str-subclass:", list(S("ab")))

# Plain (non-subclass) non-iterables are unchanged.
for obj in (42, None, 3.5):
    try:
        iter(obj)
    except TypeError as e:
        print("plain:", e)
