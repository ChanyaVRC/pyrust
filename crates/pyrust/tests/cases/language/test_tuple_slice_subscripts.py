# Tuple subscripts containing slices / Ellipsis (#3316): runtime + ast parity.
import ast


class S:
    def __getitem__(self, k):
        return ("get", k)

    def __setitem__(self, k, v):
        self.last = ("set", k, v)

    def __delitem__(self, k):
        self.last = ("del", k)


def show(k):
    if isinstance(k, tuple):
        return "(" + ", ".join(show(x) for x in k) + ("," if len(k) == 1 else "") + ")"
    if isinstance(k, slice):
        return "slice(%r, %r, %r)" % (k.start, k.stop, k.step)
    return repr(k)


s = S()
print(show(s[1:2, 3]))
print(show(s[::3, ...]))
print(show(s[:, 1]))
print(show(s[1, ::3]))
print(show(s[1:2,]))
print(show(s[1:2, 3:4:5, :]))
print(show(s[:, :]))
print(show(s[..., 1:]))
print(show(s[...,]))
print(show(s[1:2, :3, 4::5, ::6, ...]))
x = [7, 8]
print(show(s[*x, 1:2]))
print(show(s[1:2, *x]))
print(show(s[*x, 1:2,]))
i, j = 1, 4
print(show(s[i:j, i + 1 : j * 2 : 3]))
print(show(s[(1, 2)]))  # parenthesised tuple unchanged
print(show(s[1:2][0]))

# store
s[:, 1] = "v"
print(show(s.last[1]), s.last[2])
s[1:2, ::3] = "w"
print(show(s.last[1]), s.last[2])
s[1:2,] = "t"
print(show(s.last[1]), s.last[2])
s[:, :] = "u"
print(show(s.last[1]), s.last[2])
s[..., 1:2] = "e"
print(show(s.last[1]), s.last[2])

# delete
del s[:, 1]
print(show(s.last[1]))
del s[1:2, ::3, ...]
print(show(s.last[1]))
del s[1:2,]
print(show(s.last[1]))


# augmented assignment
class A:
    def __getitem__(self, k):
        print("getitem", show(k))
        return 10

    def __setitem__(self, k, v):
        print("setitem", show(k), v)


a = A()
a[:, 1] += 5
a[1:2, ::3] *= 2
a[..., :] -= 1

# single evaluation of subscript operands in augassign
calls = []


def f(n):
    calls.append(n)
    return n


a[f(1):f(2), f(3)] += 1
print(calls)

lst = [1, 2, 3]
# real containers: tuple key with slice raises like CPython
try:
    lst[1:2, 0]
except TypeError as e:
    print(type(e).__name__, e)
try:
    lst[1:2, 0] = 1
except TypeError as e:
    print(type(e).__name__, e)
try:
    del lst[1:2, 0]
except TypeError as e:
    print(type(e).__name__, e)

# dict with slice in tuple key is unhashable
d = {}
try:
    d[1:2, 3] = 1
except TypeError as e:
    print(type(e).__name__, e)

# slice in subscript tuple is a real slice object each time
class Cap:
    def __getitem__(self, k):
        return k


k = Cap()[1:2, 3:]
print(type(k[0]).__name__, k[0].start, k[0].stop, k[0].step)
print(k[1].start, k[1].stop, k[1].step)

# in comprehension / lambda / nested
print([show(S()[i:i + 1, ::2]) for i in range(2)])
g = lambda o: o[1:, ...]
print(show(g(S())[1]))
print(show(S()[1:2, 3][1][0]))


# ast parity
def dump(src, mode="eval"):
    print(ast.dump(ast.parse(src, mode=mode)))


for src in [
    "a[1:2, 3]",
    "a[::3, ...]",
    "a[:, 1]",
    "a[1, ::3]",
    "a[1:2,]",
    "a[1:2, 3:4:5, :]",
    "a[*x, 1:2]",
    "a[1:2, *x]",
    "a[...]",
    "a[..., 1:2]",
]:
    dump(src)
    print(ast.unparse(ast.parse(src, mode="eval")))

dump("a[:, 1] = v", "exec")
print(ast.unparse(ast.parse("a[:, 1] = v")))
dump("a[:, 1] += v", "exec")
print(ast.unparse(ast.parse("a[:, 1] += v")))
dump("del a[:, 1]", "exec")
print(ast.unparse(ast.parse("del a[:, 1]")))
dump("a[1:2,] = v", "exec")
print(ast.unparse(ast.parse("a[1:2,] = v")))

# compile() of an AST containing tuple slices
t = ast.parse("S()[1:2, ::3, ...]", mode="eval")
print(show(eval(compile(t, "<ast>", "eval"))[1]))


# function scope, closures, class bodies, annotations
def outer():
    c = S()
    lo = 1

    def inner(hi):
        c[lo:hi, ::2] = 5
        del c[lo:hi, ...]
        return show(c[lo:hi, hi:])

    print(inner(3), show(c.last[1]))


outer()


class Body:
    o = S()
    r = o[1:2, ::3]
    o[:, 0] = 1


print(show(Body.r[1]), show(Body.o.last[1]))

ann: "S"
print(ast.unparse(ast.parse("x: a[1:2, ::3, ...] = 1")))
print(ast.dump(ast.parse("x: a[1:2, ::3, ...]").body[0].annotation))

# syntax errors
for bad in ["a[1:2:3:4]", "a[1:2,,]", "a[,]", "a[*x:1]", "a[:, 1:] = ", "a[1:2, 3"]:
    for fn in (lambda b: compile(b, "<s>", "eval"), lambda b: ast.parse(b)):
        try:
            fn(bad)
            print("ok", bad)
        except SyntaxError:
            print("SyntaxError", bad)
