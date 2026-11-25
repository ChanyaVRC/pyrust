# CPython 3.12 parity tests for the native `keyword` module (issue #3248).

import keyword

print("all:", keyword.__all__)
print("kwlist_len:", len(keyword.kwlist))
print("kwlist:", keyword.kwlist)
print("softkwlist:", keyword.softkwlist)

for kw in ["False", "None", "True", "and", "async", "await", "def", "lambda", "yield", "nonlocal"]:
    print("is_kw", kw, keyword.iskeyword(kw), keyword.issoftkeyword(kw))

for skw in ["_", "case", "match", "type"]:
    print("is_soft", skw, keyword.iskeyword(skw), keyword.issoftkeyword(skw))

for non in ["", "foo", "print", "self", "DEF", "True ", " match"]:
    print("non", repr(non), keyword.iskeyword(non), keyword.issoftkeyword(non))

# Non-string values (including set, which frozenset.__contains__ coerces) return False
for val in [None, 0, 1, 3.14, b"def", ("def",), frozenset(["def"]), set(["def"])]:
    print("non_str_false", type(val).__name__, keyword.iskeyword(val), keyword.issoftkeyword(val))

# Unhashable values raise TypeError
for bad in [[], {}]:
    for fn_name, fn in [("iskeyword", keyword.iskeyword), ("issoftkeyword", keyword.issoftkeyword)]:
        try:
            fn(bad)
            print("UNEXPECTED_NO_ERR", fn_name, type(bad).__name__)
        except TypeError:
            print("unhashable_ok", fn_name, type(bad).__name__)

# Mutating kwlist does not affect iskeyword
orig_kw = list(keyword.kwlist)
keyword.kwlist.append("pyrust_fake_kw")
print("mutated_kwlist_check:", keyword.iskeyword("pyrust_fake_kw"))
keyword.kwlist[:] = orig_kw
