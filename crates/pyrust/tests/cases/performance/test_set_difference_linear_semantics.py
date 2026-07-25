base = set(range(2000))
diff = base.difference(range(0, 2000, 2), range(1, 1000, 2))
print("set-difference", len(diff), min(diff), max(diff), sum(diff))

base.difference_update(range(0, 2000, 3), range(1, 2000, 3))
print("set-difference-update", len(base), min(base), max(base), sum(base))

singleton = {1, 2, 3}
singleton.difference_update({2})
print("set-singleton", sorted(singleton))
