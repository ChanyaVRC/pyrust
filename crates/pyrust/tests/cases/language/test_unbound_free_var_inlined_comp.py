# Free var in inlined comprehension grandparent scope raises NameError, not
# UnboundLocalError (issue #2457). PEP 709 inlines list/set/dict comps into the
# enclosing frame, but a name from a grandparent scope is still a free var.


# free var in grandparent scope -- should NameError
def a():
    def b():
        return [x for _ in range(1)]

    try:
        b()
    except NameError as e:
        print(type(e).__name__, e)
    x = 1


a()


# local of immediate enclosing function -- should UnboundLocalError
def c():
    try:
        result = [y for _ in range(1)]  # y is local to c
    except UnboundLocalError as e:
        print(type(e).__name__, e)
    y = 1


c()


# normal free var (not in comp) -- should NameError
def d():
    def e():
        return z

    try:
        e()
    except NameError as e2:
        print(type(e2).__name__, e2)
    z = 1


d()
