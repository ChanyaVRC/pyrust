# PEP 634: mapping and unordered collection types must not match sequence
# patterns even though they support len().  CPython uses tp_as_sequence to
# make this distinction; pyrust replicates it via an isinstance guard.

# dict — falls through (d[0] would raise KeyError)
match {"a": 1}:
    case [x]:
        print("FAIL: dict matched sequence")
    case _:
        print("dict: no match")

# empty dict
match {}:
    case [x]:
        print("FAIL: empty dict matched sequence")
    case _:
        print("empty dict: no match")

# set — falls through (s[0] raises TypeError)
match {1, 2}:
    case [x]:
        print("FAIL: set matched sequence")
    case _:
        print("set: no match")

# empty set literal isn't valid syntax; use set()
match set():
    case []:
        print("FAIL: empty set matched sequence")
    case _:
        print("empty set: no match")

# frozenset — falls through
match frozenset():
    case []:
        print("FAIL: frozenset matched sequence")
    case _:
        print("frozenset: no match")

match frozenset({1, 2}):
    case [x]:
        print("FAIL: frozenset matched sequence")
    case _:
        print("frozenset(nonempty): no match")

# list — must still match
match [42]:
    case [x]:
        print(f"list: matched x={x}")
    case _:
        print("FAIL: list did not match")

# tuple — must still match
match (10, 20):
    case [a, b]:
        print(f"tuple: matched a={a} b={b}")
    case _:
        print("FAIL: tuple did not match")

# str — excluded per PEP 634
match "hi":
    case [x]:
        print("FAIL: str matched sequence")
    case _:
        print("str: no match")

# bytes — excluded per PEP 634
match b"hi":
    case [x]:
        print("FAIL: bytes matched sequence")
    case _:
        print("bytes: no match")

# star pattern still works for list
match [1, 2, 3]:
    case [first, *rest]:
        print(f"star: first={first} rest={rest}")
    case _:
        print("FAIL: star pattern on list failed")

print("done")
