# dict() update-sequence error messages must match CPython 3.12 exactly:
#   - non-sequence element -> TypeError with element index #N
#   - wrong-length element  -> ValueError with element index #N


def show(expr, thunk):
    try:
        thunk()
        print(expr, "-> no error")
    except Exception as e:
        print(expr, "->", type(e).__name__ + ":", e)


# --- non-sequence element: "cannot convert ... element #N to a sequence" ---
show("dict([1, 2, 3])", lambda: dict([1, 2, 3]))
show("dict([1, (2, 3)])", lambda: dict([1, (2, 3)]))

# --- wrong-length element: index points at the failing element ---
show("dict([(1, 2, 3)])", lambda: dict([(1, 2, 3)]))
show("dict([(1, 2), (3, 4, 5)])", lambda: dict([(1, 2), (3, 4, 5)]))
show("dict([()])", lambda: dict([()]))
show('dict(["abc"])', lambda: dict(["abc"]))

# --- an error raised *inside* an element's own iteration propagates
#     unchanged (it must NOT be rewritten to "cannot convert ...") ---
class BadIter:
    def __iter__(self):
        raise RuntimeError("boom")


show("dict([BadIter()])", lambda: dict([BadIter()]))

# --- happy paths still work ---
print(dict([(1, 2)]))
print(dict([(1, 2), (3, 4)]))
print(dict(["ab", "cd"]))
print(dict({"a": 1}))
print(dict())
