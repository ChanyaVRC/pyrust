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

print("starred assign OK")
