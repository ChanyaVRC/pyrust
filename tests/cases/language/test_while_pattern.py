# while-compare-increment pattern
i = 0
s = 0
while i < 100:
    s += i
    i += 1
print(s)  # 4950
print(i)  # 100

# Small range unrolling
total = 0
for x in range(5):
    total += x * x
print(total)  # 0+1+4+9+16 = 30

# while with step > 1
i = 0
count = 0
while i < 20:
    count += 1
    i += 3
print(count)  # 7
print(i)      # 21
