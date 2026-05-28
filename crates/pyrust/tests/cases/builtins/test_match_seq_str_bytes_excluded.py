# PEP 634 §3: str and bytes must NOT match sequence patterns in match/case.
# CPython 3.12 rejects them via Py_TPFLAGS_SEQUENCE / MATCH_SEQUENCE exclusion.

# str: non-empty, with star
match "hello":
    case [a, *rest]:
        print("matched str")
    case _:
        print("str not matched")

# str: empty
match "":
    case []:
        print("matched empty str")
    case _:
        print("empty str not matched")

# str: single char
match "x":
    case [c]:
        print("matched single char str")
    case _:
        print("str x not matched")

# bytes: non-empty, with star
match b"hello":
    case [a, *rest]:
        print("matched bytes")
    case _:
        print("bytes not matched")

# bytes: empty
match b"":
    case []:
        print("matched empty bytes")
    case _:
        print("empty bytes not matched")

# bytes: single element
match b"x":
    case [c]:
        print("matched single byte")
    case _:
        print("bytes x not matched")

# list subjects SHOULD still match
match [1, 2, 3]:
    case [a, *rest]:
        print(f"list matched: a={a}, rest={rest}")

# tuple subjects SHOULD still match
match (1, 2, 3):
    case [a, *rest]:
        print(f"tuple matched: a={a}, rest={rest}")

# empty list and empty tuple SHOULD still match
match []:
    case []:
        print("empty list matched")

match ():
    case []:
        print("empty tuple matched")
