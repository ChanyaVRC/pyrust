def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(10))

total = 0
for i in range(100):
    total += i
print(total)

xs = [x * x for x in range(5)]
print(xs)
