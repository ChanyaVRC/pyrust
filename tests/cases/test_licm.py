# Invariant flag loop
done = False
count = 0
while not done:
    count += 1
    if count >= 100:
        break
print(count)  # 100

# Standard incrementing while (not LICM eligible — i changes)
i = 0
total = 0
while i < 50:
    total += i
    i += 1
print(total)  # 1225

# Invariant condition that's False from the start
x = 0
result = 99
while x:
    result = 0
print(result)  # 99 (body never ran)
