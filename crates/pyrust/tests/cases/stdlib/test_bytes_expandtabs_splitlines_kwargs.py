# bytes/bytearray expandtabs(tabsize=) and splitlines(keepends=) accept their
# argument by keyword as well as position (#1990). CPython 3.12 parity for both
# receivers, both forms, and the TypeError paths.

for ctor in (bytes, bytearray):
    name = ctor.__name__

    # --- expandtabs ---
    v = ctor(b"a\tb")
    print(name, "expandtabs kw:", v.expandtabs(tabsize=4))
    print(name, "expandtabs pos:", v.expandtabs(4))
    print(name, "expandtabs default:", v.expandtabs())
    print(name, "expandtabs tabsize=0:", v.expandtabs(tabsize=0))
    print(name, "expandtabs tabsize=1:", v.expandtabs(tabsize=1))
    print(name, "expandtabs tabsize=True:", ctor(b"a").expandtabs(tabsize=True))

    # --- splitlines ---
    w = ctor(b"x\ny")
    print(name, "splitlines keepends=True:", w.splitlines(keepends=True))
    print(name, "splitlines keepends=False:", w.splitlines(keepends=False))
    print(name, "splitlines keepends=1:", w.splitlines(keepends=1))
    print(name, "splitlines keepends=2:", w.splitlines(keepends=2))
    print(name, "splitlines default:", w.splitlines())
    print(name, "splitlines pos:", w.splitlines(True))

    # bytes/bytearray splitlines only break on \r, \n, \r\n (not \v \f \x1c…).
    mixed = ctor(b"a\rb\nc\r\nd\ve\ff\x1cg")
    print(name, "splitlines boundaries:", mixed.splitlines())
    print(name, "splitlines boundaries keepends:", mixed.splitlines(keepends=True))


def err(label, fn):
    try:
        fn()
        print(label, "-> NO ERROR")
    except TypeError as e:
        print(label, "-> TypeError:", e)


for ctor in (bytes, bytearray):
    name = ctor.__name__
    v = ctor(b"a")
    err(name + " expandtabs bad kw", lambda: v.expandtabs(foo=4))
    err(name + " expandtabs pos+kw", lambda: v.expandtabs(4, tabsize=4))
    err(name + " expandtabs too many pos", lambda: v.expandtabs(4, 5))
    err(name + " expandtabs bad type", lambda: v.expandtabs(tabsize="x"))
    err(name + " expandtabs two kw", lambda: v.expandtabs(tabsize=4, foo=1))
    err(name + " splitlines bad kw", lambda: v.splitlines(foo=4))
    err(name + " splitlines pos+kw", lambda: v.splitlines(True, keepends=True))
    err(name + " splitlines too many pos", lambda: v.splitlines(1, 2))
    err(name + " splitlines two kw", lambda: v.splitlines(keepends=True, foo=1))
