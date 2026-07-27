target = 41
for target in range(0):
    never_bound = target + 1

print(target)
print("never_bound" in globals())

total = 0
for index in range(5):
    total += index

print(index, total)
print(globals()["index"], globals()["total"])

for insertion_index in range(1):
    first_inserted = insertion_index
    second_inserted = insertion_index + 1

interesting = {"first_inserted", "second_inserted"}
print([name for name in globals() if name in interesting])

negative_total = 0
for negative_index in range(5, -1, -2):
    negative_total += negative_index
print(negative_index, negative_total)

big_total = 0
for big_index in range(10**30, 10**30 + 2):
    big_total += big_index
print(big_index, big_total)


def range(stop):
    return [7, 8][:stop]


shadow_total = 0
for shadow_index in range(2):
    shadow_total += shadow_index
print(shadow_index, shadow_total)

flow = []
for flow_index in [0, 1, 2, 3]:
    if flow_index == 1:
        continue
    if flow_index == 3:
        break
    flow.append(flow_index)
else:
    flow.append("else")
print(flow)
