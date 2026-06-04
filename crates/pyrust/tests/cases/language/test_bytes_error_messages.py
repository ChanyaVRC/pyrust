# bytes/bytearray split + startswith/endswith TypeError wording (issue #2044).
# Wrong-type arguments must use CPython 3.12's canonical
# "a bytes-like object is required, not '<type>'" message (naming the offending
# type), while the single non-tuple startswith/endswith arg keeps the distinct
# "first arg must be bytes or a tuple of bytes, not <type>" message.


def show(label, fn):
    try:
        print(label, "OK", fn())
    except TypeError as e:
        print(label, "TypeError", str(e))


# split / rsplit with a non-bytes separator name the offending type.
show("split str", lambda: b"a b".split("x"))
show("split int", lambda: b"a b".split(5))
show("rsplit str", lambda: b"a b".rsplit("x"))
show("ba split str", lambda: bytearray(b"a b").split("x"))

# startswith/endswith with a tuple containing a non-bytes element.
show("sw tuple str-first", lambda: b"abc".startswith(("x", 1)))
show("sw tuple int-first", lambda: b"abc".startswith((1, b"a")))
show("sw tuple int-after-bytes", lambda: b"abc".startswith((b"z", 1)))
show("sw tuple float", lambda: b"abc".startswith((1.5,)))
show("ew tuple str", lambda: b"abc".endswith(("x", 1)))
show("ba sw tuple str", lambda: bytearray(b"abc").startswith(("x", 1)))

# A single (non-tuple) non-bytes first arg keeps the other message.
show("sw single int", lambda: b"abc".startswith(5))
show("sw single str", lambda: b"abc".startswith("a"))
show("ew single int", lambda: b"abc".endswith(5))
show("ba sw single int", lambda: bytearray(b"abc").startswith(5))

# Valid calls are unaffected.
show("split bytes", lambda: b"a b".split(b" "))
show("split none", lambda: b"a b c".split())
show("split bytearray sep", lambda: b"a-b".split(bytearray(b"-")))
show("sw ok", lambda: b"abc".startswith(b"a"))
show("sw ok tuple", lambda: b"abc".startswith((b"z", b"a")))
show("sw ok bytearray tuple", lambda: b"abc".startswith((bytearray(b"a"),)))
show("ew ok", lambda: b"abc".endswith(b"c"))
