# Builtins that accept no keyword arguments must say
# "FUNC() takes no keyword arguments", not "got an unexpected keyword
# argument 'X'".  CPython 3.12 parity test.
fns = [
    (lambda: len(x=1), "len"),
    (lambda: abs(x=1), "abs"),
    (lambda: repr(x=1), "repr"),
    (lambda: id(x=1), "id"),
    (lambda: hash(x=1), "hash"),
    (lambda: hex(x=1), "hex"),
    (lambda: oct(x=1), "oct"),
    (lambda: bin(x=1), "bin"),
    (lambda: ord(x="a"), "ord"),
    (lambda: chr(x=65), "chr"),
    (lambda: callable(x=1), "callable"),
    (lambda: iter(x=[]), "iter"),
    (lambda: bool(x=1), "bool"),
    (lambda: dir(x=1), "dir"),
]
for fn, name in fns:
    try:
        fn()
    except TypeError as e:
        msg = str(e)
        if "takes no keyword arguments" in msg:
            print(f"{name}: ok")
        else:
            print(f"{name}: WRONG: {msg}")
