result = 0
for _ in range(500):
    a = 0
    b = 1
    for i in range(85):
        a, b = b, a + b
    result = a
assert result == 259695496911122585
