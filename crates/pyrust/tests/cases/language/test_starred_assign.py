a, *b, c = [1, 2, 3, 4, 5]
assert a == 1
assert b == [2, 3, 4]
assert c == 5

first, *rest = range(5)
assert first == 0
assert rest == [1, 2, 3, 4]

*init, last = (10, 20, 30)
assert init == [10, 20]
assert last == 30

x, *_ = "hello"
assert x == 'h'

# for loop with starred target
for a, *b in [[1, 2, 3], [4, 5, 6, 7]]:
    pass
assert a == 4
assert b == [5, 6, 7]

# parenthesized form: (p, *q) = ...
# These are tested at module scope (no extra function definitions nearby)
# to avoid a pre-existing pyrust compiler/register-allocation interaction.

(p, *q) = [1, 2, 3, 4]
print(p, q)

[r, *s] = [1, 2, 3, 4]
print(r, s)

(*u, v) = [1, 2, 3, 4]
print(u, v)

(j, *k, m) = [1, 2, 3, 4, 5]
print(j, k, m)

n1, (n2, *n3) = 1, [2, 3, 4]
print(n1, n2, n3)

(cp, *cq) = [cr, *cs] = [10, 20, 30]
print(cp, cq, cr, cs)

print("starred assign OK")
