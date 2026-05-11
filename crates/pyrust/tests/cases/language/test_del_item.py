# del lst[i]: front, middle, and end deletion
lst = [1, 2, 3, 4, 5]
del lst[2]
print(lst)   # [1, 2, 4, 5]

del lst[0]
print(lst)   # [2, 4, 5]

del lst[-1]
print(lst)   # [2, 4]

# del on a single-element list
solo = [42]
del solo[0]
print(solo)  # []

# del d[k]: dict deletion
d = {"a": 1, "b": 2, "c": 3}
del d["b"]
print(len(d))    # 2
print(d["a"])    # 1
print(d["c"])    # 3
