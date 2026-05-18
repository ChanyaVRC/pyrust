# Generator expression syntax: (elt for target in iter [if cond] [for ...])

# Basic form: sum over a generator
print(sum(x * x for x in range(4)))  # 14

# Genexp passed to list()
print(list(x for x in [1, 2, 3]))  # [1, 2, 3]

# Standalone genexp assigned to variable, consumed by list()
g = (x for x in range(3))
print(list(g))  # [0, 1, 2]

# Genexp with if-filter
print(list(x * 2 for x in range(3) if x > 0))  # [2, 4]

# Genexp in any() / all()
print(any(x > 2 for x in range(5)))  # True
print(all(x > 0 for x in range(1, 4)))  # True

# Genexp in next()
print(next(x for x in range(10) if x > 5))  # 6

# Genexp with empty source
print(list(x for x in []))  # []

# Genexp is lazy — can iterate a very large range without materialising it
g2 = (x for x in range(1000000))
print(next(g2))  # 0
print(next(g2))  # 1

# Nested for-clauses
print(list(x + y for x in range(3) for y in range(2)))  # [0, 1, 1, 2, 2, 3]

# Parenthesised genexp form (not a tuple)
result = (x * x for x in range(4))
print(type(result).__name__)  # generator
print(list(result))  # [0, 1, 4, 9]

# Genexp body references a variable from the enclosing scope
offset = 100
print(list(offset + i for i in range(3)))  # [100, 101, 102]

# Genexp inside a function closes over the function's local
def make_gen(base):
    return (base + i for i in range(3))
print(list(make_gen(10)))  # [10, 11, 12]

# Lazy evaluation: body must not execute until next() is called
_lazy_called = []
def _lazy_f():
    _lazy_called.append(1)
    return 42
g3 = (_lazy_f() for _ in range(3))
print(len(_lazy_called))  # 0  (not yet called)
next(g3)
print(len(_lazy_called))  # 1  (called exactly once)
