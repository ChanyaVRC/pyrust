# str.index/count/find/rfind: first arg must be a str.
# Valid results (including start/end) and the TypeError raised for a
# non-str first argument must match CPython 3.12 byte-for-byte.
s = "hello world"

# happy path
print(s.index("o"))
print(s.index("o", 5, 11))
print(s.count("o"))
print(s.count("l", 3))
print(s.find("o"))
print(s.find("z"))
print(s.rfind("o"))
print(s.rfind("z"))

# non-str first argument -> TypeError "must be str, not <type>"
for method in ("index", "count", "find", "rfind"):
    f = getattr(s, method)
    for bad in (1, None, [1]):
        try:
            f(bad)
        except TypeError as e:
            print(method, type(bad).__name__, str(e))
