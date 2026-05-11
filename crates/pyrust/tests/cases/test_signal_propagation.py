def find_first(lst, pred):
    for i, x in enumerate(lst):
        if pred(x):
            return i
    return -1

result = find_first([1, 4, 7, 2, 9], lambda x: x > 6)
print(result)  # 2

def sum_until(n, limit):
    s = 0
    for i in range(n):
        s += i
        if s > limit:
            return s
    return s

print(sum_until(100, 50))  # first s > 50: s = 55 (after adding 10+11 = 55 when i=10)
