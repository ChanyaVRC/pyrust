# Automatic memoization of pure scalar functions (#2234): must be observably
# transparent — same results as without memoization, for every shape below.

# Pure recursion (memoized): exponential fib collapses but the value is exact.
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)
print([fib(i) for i in range(20)])
print(fib(30))

def fact(n):
    if n == 0:
        return 1
    return n * fact(n - 1)
print([fact(i) for i in range(12)])

# Ackermann (pure, two int args) — exercises multi-arg keys.
def ack(m, n):
    if m == 0:
        return n + 1
    if n == 0:
        return ack(m - 1, 1)
    return ack(m - 1, ack(m, n - 1))
print(ack(2, 3), ack(3, 3))

# A function that READS A GLOBAL is impure ⇒ NOT memoized: after the global
# changes the result must change (no stale cache).
G = 10
def reads_global(n):
    return n + G
print(reads_global(5))
G = 100
print(reads_global(5))

H = 1
def fib_g(n):
    if n <= 1:
        return n + H
    return fib_g(n - 1) + fib_g(n - 2)
print(fib_g(10))
H = 1000
print(fib_g(10))

# Calling the same pure function with the SAME args many times: value stable.
def square(n):
    return n * n
total = 0
for _ in range(1000):
    total += square(7)
print(total)

# Negative and zero args.
def tri(n):
    if n <= 0:
        return 0
    return n + tri(n - 1)
print(tri(0), tri(1), tri(50), tri(-5))

# A pure function returning a non-scalar (tuple) must still be correct (it is
# simply not cached — identity of fresh tuples is preserved).
def pair(n):
    if n == 0:
        return (0,)
    return pair(n - 1)
a = pair(3)
b = pair(3)
print(a == b, a is b)

# Boolean-returning pure recursion.
def is_even(n):
    if n == 0:
        return True
    if n == 1:
        return False
    return is_even(n - 2)
print(is_even(10), is_even(7), is_even(100))
