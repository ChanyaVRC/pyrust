BIG = 10**100


def show(fn):
    try:
        fn()
        print("NO ERROR")
    except Exception as e:
        print(type(e).__name__, e)


# str.split / rsplit / replace
show(lambda: "a b c".split(" ", BIG))
show(lambda: "a b c".rsplit(" ", BIG))
show(lambda: "hello hello".replace("hello", "HI", BIG))

# bytes.split / rsplit / replace
show(lambda: b"a b c".split(b" ", BIG))
show(lambda: b"a b c".rsplit(b" ", BIG))
show(lambda: b"hello".replace(b"h", b"H", BIG))

# bytearray.split / rsplit / replace
show(lambda: bytearray(b"a b c").split(b" ", BIG))
show(lambda: bytearray(b"a b c").rsplit(b" ", BIG))
show(lambda: bytearray(b"hello").replace(b"h", b"H", BIG))

# keyword form for maxsplit (str + bytes)
show(lambda: "a b c".split(" ", maxsplit=BIG))
show(lambda: b"a b c".split(sep=b" ", maxsplit=BIG))

# negative BigInt also overflows (cannot fit ssize_t)
show(lambda: "a b c".split(" ", -(10**100)))
