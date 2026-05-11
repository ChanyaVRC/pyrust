# Basic walrus
x = None
if (x := 10) > 5:
    assert x == 10

# In while loop
data = [1, 2, 3, 4, 5]
results = []
i = 0
while (val := data[i] if i < len(data) else None) is not None:
    results.append(val)
    i += 1
assert results == [1, 2, 3, 4, 5]

# In list comprehension (assigns to enclosing scope)
nums = [1, -2, 3, -4, 5]
positives = [y for x in nums if (y := x) > 0]
assert positives == [1, 3, 5]

# Walrus in a condition — value persists after loop
found = None
for item in [1, 2, 3]:
    if (found := item) == 2:
        break
assert found == 2

print("walrus OK")
