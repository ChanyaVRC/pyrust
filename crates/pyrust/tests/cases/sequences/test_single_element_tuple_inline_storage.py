# Pins tuple semantics for the 1-element shape, which is stored inline like
# the 2- and 3-element shapes since issue #3426 (it is the `args` of every
# exception constructed with one argument).  The neighbouring lengths are
# exercised alongside so the representation boundary stays invisible.
import copy
import pickle

print("== construction and equality ==")
for n in range(5):
    t = tuple(range(n))
    print(n, t, len(t), repr(t), str(t), bool(t), t == tuple(list(t)), hash(t) == hash(tuple(range(n))))

t = (1,)
print(t, t[0], t[-1], t == (1,), t != (1,), t == (1, 2), t == [1], t == 1)
print((1,) < (2,), (1,) < (1, 0), (1,) <= (1,), (2,) > (1, 9), ((),) == ((),))
print(tuple([5]), tuple("a"), tuple({7}), tuple(x for x in [8]), tuple(tuple([9])))

print("== identity ==")
item = object()
a = (item,)
b = a
c = (item,)
print(a is b, a[0] is c[0], id(a) == id(b), a == c, type(a) is tuple, type(a).__name__)
print(a[:] is a, (a + ()) == a, copy.copy(a) is a, copy.deepcopy((1,)) == (1,))

print("== sequence protocol ==")
print(t + (2,), (0,) + t, t + (), t * 3, 3 * t, t * 0, list(t), sorted(t), tuple(reversed(t)))
print(t[::-1], t[0:1], t[1:], t[:0], t[-1:], t[::2], 1 in t, 2 in t, t.count(1), t.index(1))
print(min(t), max(t), sum(t), any(t), all(t), len(t * 4), list(enumerate(t)), list(zip(t, t)))
for value in t:
    print("iter", value)
try:
    t[1]
except IndexError as err:
    print("IndexError", err)
try:
    t.index(9)
except ValueError as err:
    print("ValueError", err)
try:
    t[0] = 2
except TypeError as err:
    print("TypeError", err)

print("== hashing and keys ==")
d = {(1,): "one", (1, 2): "two", (): "empty"}
print(d[(1,)], d[(1, 2)], d[()], (1,) in d, (2,) in d, sorted(d, key=len))
s = {(1,), (1,), (2,), (1.0,), (True,)}
print(len(s), (1,) in s, sorted(s))
print({(1,): 1} == {(1.0,): 1}, hash((1,)) == hash((1.0,)), hash((1,)) == hash((True,)))
try:
    hash(([],))
except TypeError as err:
    print("TypeError", err)

print("== unpacking and calls ==")
(only,) = (9,)
[alone] = (8,)
first, *rest = (7,)
print(only, alone, first, rest)


def star(*args, **kwargs):
    return args, kwargs


print(star(1), star(*(2,)), star(*[3]), star(*(4,), *(5,)), star(None)[0] == (None,))
print([x for (x,) in [(1,), (2,)]], "%s" % (7,), "%s-%s" % ((1,), 2), "{}".format(*(8,)))
print(isinstance(1, (int,)), issubclass(bool, (int,)), isinstance("s", (int,)))
try:
    a, b = (1,)
except ValueError as err:
    print("ValueError", err)

print("== nesting ==")
nested = (((1,),),)
print(nested, nested[0][0][0], len(nested), nested == (((1,),),), [(x,) for x in range(3)])
print(((),), ((),)[0], len(((),)), ([],)[0], ({},), (None,), ((1, 2),), (((1,), (2,)),))

print("== exceptions carry the argument tuple ==")
print(ValueError("only").args, KeyError(("k",)).args, StopIteration((1,)).value, Exception((1,)).args)
try:
    raise ValueError("x")
except (ValueError,):
    print("tuple-of-one handler")
try:
    raise KeyError((1,))
except KeyError as err:
    print(repr(err), str(err), err.args[0], err.args == ((1,),))

print("== copying and pickling ==")
print(pickle.loads(pickle.dumps((1,))), pickle.loads(pickle.dumps(((1,), [2]))), copy.deepcopy(([1],)))
mutable = ([1],)
clone = copy.deepcopy(mutable)
clone[0].append(2)
print(mutable, clone, mutable[0] is not clone[0])

print("== subclass and conversion ==")


class Point(tuple):
    pass


p = Point((3,))
print(p, type(p).__name__, p == (3,), tuple(p), len(p), p[0], isinstance(p, tuple), type(tuple(p)).__name__)
print(str((1,)), repr(("a",)), f"{(1,)}", "%r" % ((1,),), list(map(tuple, ["a", "bc"])))
