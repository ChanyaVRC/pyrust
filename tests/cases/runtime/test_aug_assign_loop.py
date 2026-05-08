total = 0
for i in range(100):
    total += i
print(total)  # 4950

s = 0
for i in range(1, 50, 2):
    s += i
print(s)  # sum of odd numbers 1..49

# step=1 range with else
result = 0
for i in range(10):
    result += i
else:
    result += 100
print(result)  # 45 + 100 = 145
