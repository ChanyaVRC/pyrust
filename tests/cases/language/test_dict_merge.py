# DictUpdate instruction: **kwargs merging in function calls (non-overlapping keys)
def merge(**kw):
    return kw

a = {"x": 1, "y": 2}
b = {"z": 3, "w": 4}
result = merge(**a, **b)
print(result["x"])   # 1
print(result["y"])   # 2
print(result["z"])   # 3
print(result["w"])   # 4

# Empty dicts
empty = merge(**{}, **{})
print(empty)         # {}

# Single splat
single = merge(**{"k": 7})
print(single["k"])   # 7
