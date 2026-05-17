def outer():
    x = 1
    def inner():
        nonlocal x
        x = 2
        locs = locals()
        print('x' in locs)   # True
        print(locs['x'])      # 2
    inner()

outer()

# nonlocal var read (not written) in inner
def outer2():
    a = 10
    def inner2():
        nonlocal a
        return locals()
    locs = inner2()
    print('a' in locs)  # True
    print(locs['a'])    # 10

outer2()

# locals() still works for normal (non-nonlocal) functions
def normal():
    b = 99
    return locals()
print(normal()['b'])    # 99

# Multiple nonlocal names
def outer3():
    a = 1
    b = 2
    def inner3():
        nonlocal a, b
        a = 10
        b = 20
        locs = locals()
        print(sorted(locs.items()))
    inner3()

outer3()

# Mixed nonlocal + fastlocal in same function
def outer4():
    x = 100
    def inner4():
        nonlocal x
        y = 200
        locs = locals()
        print(sorted(locs.items()))
    inner4()

outer4()
