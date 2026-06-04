# parity fixture (#2079): str.encode() must reject lone surrogates under strict
# and honour the errors handler for utf-8 / utf-16 / utf-32.


def show(fn):
    try:
        print("OK", repr(fn()))
    except Exception as e:
        attrs = ""
        if isinstance(e, UnicodeEncodeError):
            attrs = " start=%d end=%d reason=%r" % (e.start, e.end, e.reason)
        print("ERR", type(e).__name__, repr(str(e)) + attrs)


# --- strict (default) raises UnicodeEncodeError with correct attrs ---
show(lambda: "\ud800".encode("utf-8"))
show(lambda: "a\ud800b".encode("utf-8"))
show(lambda: "\ud800".encode("utf-16"))
show(lambda: "\ud800".encode("utf-32"))
show(lambda: "\ud800".encode("utf-16-le"))
show(lambda: "\ud800".encode("utf-16-be"))
show(lambda: "\ud800".encode("utf-32-le"))
show(lambda: "\ud800".encode("utf-32-be"))
# consecutive surrogates coalesce into one error run
show(lambda: "\ud800\ud801".encode("utf-8"))
show(lambda: "pre𐏿post".encode("utf-8"))

# --- surrogatepass emits the raw surrogate bytes ---
print(repr("\ud800".encode("utf-8", "surrogatepass")))
print(repr("\ud800".encode("utf-16-le", "surrogatepass")))
print(repr("\ud800".encode("utf-16-be", "surrogatepass")))
print(repr("\ud800".encode("utf-32-le", "surrogatepass")))
print(repr("a\ud800b".encode("utf-8", "surrogatepass")))

# --- replace / ignore / backslashreplace / xmlcharrefreplace / namereplace ---
print(repr("\ud800".encode("utf-8", "ignore")))
print(repr("x\ud800y".encode("utf-8", "replace")))
print(repr("\ud800".encode("utf-8", "backslashreplace")))
print(repr("\ud800".encode("utf-8", "xmlcharrefreplace")))
print(repr("\ud800".encode("utf-8", "namereplace")))

# --- non-surrogate strings encode identically (no regression) ---
print(repr("café".encode("utf-8")))
print(repr("café".encode("utf-16-le")))
print(repr("café".encode("utf-32-le")))
print(repr("\U0001F600".encode("utf-8")))
print(repr("\U0001F600".encode("utf-16-le")))
print(repr("hello".encode("utf-8")))

# --- handler not consulted when there is no error (lazy validation) ---
print(repr("abc".encode("utf-8", "definitely-not-a-handler")))
# ... but it is consulted when a surrogate is present
show(lambda: "\ud800".encode("utf-8", "definitely-not-a-handler"))
