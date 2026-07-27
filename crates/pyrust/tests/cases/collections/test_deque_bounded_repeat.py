from collections import deque


base = deque(range(4), maxlen=5)
print("large:", list(base * (10**9)), (base * (10**9)).maxlen)
print("reflected:", list(3 * base))
print("zero:", list(base * 0), (base * 0).maxlen)
print("negative:", list(base * -4))

short = deque([1, 2], maxlen=10)
print("below-bound:", list(short * 3))

zero_bound = deque([1, 2], maxlen=0)
print("zero-bound:", list(zero_bound * (10**9)), (zero_bound * (10**9)).maxlen)
