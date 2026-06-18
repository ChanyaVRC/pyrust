# Augmented-assignment TypeError messages use the in-place operator symbol
# (`+=`, `-=`, `**=`, …), matching CPython 3.12 — issue #2561.


def show(label, f):
    try:
        f()
    except TypeError as e:
        print(label, "->", str(e))


# --- property setter (the original repro) ---------------------------------
class C:
    @property
    def foo(self):
        return 1

    @foo.setter
    def foo(self, v):
        pass


def prop_aug():
    c = C()
    c.foo += "s"


show("prop +=", prop_aug)


# --- every in-place operator on a plain local -----------------------------
def aug_add():
    x = 1
    x += "s"


def aug_sub():
    x = 1
    x -= "s"


def aug_mul():
    x = 1
    x *= set()


def aug_div():
    x = 1
    x /= "s"


def aug_floordiv():
    x = 1
    x //= "s"


def aug_mod():
    x = 1
    x %= set()


def aug_pow():
    x = 2
    x **= "s"


def aug_and():
    x = 1
    x &= "s"


def aug_or():
    x = 1
    x |= "s"


def aug_xor():
    x = 1
    x ^= "s"


def aug_lshift():
    x = 1
    x <<= "s"


def aug_rshift():
    x = 1
    x >>= "s"


def aug_matmul():
    x = 1
    x @= 2


show("aug +=", aug_add)
show("aug -=", aug_sub)
show("aug *=", aug_mul)
show("aug /=", aug_div)
show("aug //=", aug_floordiv)
show("aug %=", aug_mod)
show("aug **=", aug_pow)
show("aug &=", aug_and)
show("aug |=", aug_or)
show("aug ^=", aug_xor)
show("aug <<=", aug_lshift)
show("aug >>=", aug_rshift)
show("aug @=", aug_matmul)


# --- variable RHS (BinOpInPlace path, not const-folded) -------------------
def aug_var_rhs():
    x = 1
    y = "s"
    x += y


show("var +=", aug_var_rhs)


# --- attribute / subscript targets ----------------------------------------
class D:
    pass


def attr_aug():
    d = D()
    d.v = 1
    d.v += "s"


def index_aug():
    m = {0: 1}
    m[0] += "s"


show("attr +=", attr_aug)
show("index +=", index_aug)


# --- regression guard: plain binary keeps the base symbol -----------------
show("plain +", lambda: 1 + "s")
show("plain -", lambda: 1 - "s")
show("plain **", lambda: 2 ** "s")
show("plain //", lambda: 1 // "s")


# --- regression guard: sequence-specific messages are NOT suffixed --------
def str_aug_int():
    x = "a"
    x += 5


def list_aug_int():
    x = [1]
    x += 5


show("str += int", str_aug_int)
show("list += int", list_aug_int)
