d = {"a": 1, "b": 2}

# normal deletion works
del d["a"]
print(d)       # {'b': 2}

# missing string key raises KeyError
try:
    del d["nonexistent"]
except KeyError as e:
    print(type(e).__name__, str(e))   # KeyError 'nonexistent'

# int key deletion works
d2 = {1: "one", 2: "two"}
del d2[1]
print(d2)      # {2: 'two'}

# missing int key raises KeyError
try:
    del d2[99]
except KeyError as e:
    print(type(e).__name__, str(e))   # KeyError 99
