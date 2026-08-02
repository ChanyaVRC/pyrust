# Parity fixture for #2843: call-site * / ** expansion errors must use the
# callee-aware CPython 3.12 diagnostics, while mapping-protocol and arity
# behaviour stays shared with the surrounding call machinery.

from collections import Counter


def run(label, fn):
    try:
        fn()
        print(f"{label}: <no error>")
    except BaseException as error:
        print(f"{label}: {type(error).__name__}: {error}")


def show(label, fn):
    try:
        print(f"{label}: {fn()}")
    except BaseException as error:
        print(f"{label}: {type(error).__name__}: {error}")


def f(a=1, b=2):
    pass


def g(a, b, *, c):
    pass


# The six issue rows.
run("1_star_int", lambda: f(*5))
run("2a_dstar_int", lambda: f(**5))
run("2b_dstar_list", lambda: f(**[1]))


class KeysNoGetitem:
    def keys(self):
        return ["x"]


class KeysEmpty:
    def keys(self):
        return []


run("3a_keys_nonempty_nogetitem", lambda: f(**KeysNoGetitem()))
run("3b_keys_empty_nogetitem", lambda: f(**KeysEmpty()))
run("4_arity_kwonly", lambda: g(*[1, 2, 3], c=3))


# Star/dstar errors use module-qualified function qualnames, including methods
# and nested functions.
class C:
    marker = "original"

    def method(self, value=1):
        return self.marker, value


def outer():
    def inner(value=1):
        pass

    return inner


inner = outer()
run("5_method_star", lambda: C().method(*5))
run("6_nested_dstar", lambda: inner(**5))


# The receiver is evaluated before argument expressions. Rebinding the local
# used by the receiver must neither erase its diagnostic name nor redirect a
# successful call to the replacement value.
def method_rebind_star():
    obj = C()
    return obj.method(*(obj := 5))


def method_rebind_dstar():
    obj = C()
    return obj.method(**(obj := 5))


def method_rebind_success():
    obj = C()
    return obj.method(*(obj := [7]))


run("6a_method_rebind_star", method_rebind_star)
run("6b_method_rebind_dstar", method_rebind_dstar)
show("6c_method_rebind_success", method_rebind_success)
run("6d_builtin_method_star", lambda: [].append(*5))
run("6e_builtin_method_dstar", lambda: {}.update(**5))


# A user implementation can deliberately raise the same text as the generic
# non-iterable error.  The call boundary must preserve that user exception
# instead of mistaking its text for protocol-acquisition failure.
class UserIterError:
    def __iter__(self):
        raise TypeError("'UserIterError' object is not iterable")


class UserGetitemError:
    def __getitem__(self, index):
        raise TypeError("'UserGetitemError' object is not iterable")


run("6f_user_iter_error", lambda: f(*UserIterError()))
run("6g_user_getitem_error", lambda: f(*UserGetitemError()))
run("6h_builtin_star", lambda: len(*5))


# A subscript protocol without keys() is not enough to make a ** operand a
# mapping. Non-string keys and duplicate-keyword errors retain their existing
# call-specific diagnostics.
class GetitemNoKeys:
    def __getitem__(self, key):
        return 1


class NonStringKeys:
    def keys(self):
        return [1]

    def __getitem__(self, key):
        return 1


run("7_getitem_without_keys", lambda: f(**GetitemNoKeys()))
run("8_non_string_key", lambda: f(**NonStringKeys()))
run("9_duplicate_key", lambda: f(**{"a": 1}, **{"a": 2}))


# The keyword-only suffix counts supplied keyword-only arguments, including
# singular/plural agreement, and is absent when none was supplied.
def h(a, b, *, c, d):
    pass


def zero_pos(*, only):
    pass


run("10_arity_kwonly_plural", lambda: h(*[1, 2, 3], c=3, d=4))
run("11_arity_no_kwonly_given", lambda: g(*[1, 2, 3]))
run("11b_arity_one_positional", lambda: zero_pos(*[1], only=2))


# mapping_pairs_via_protocol is shared with dict.update and dict-display
# expansion: an empty keys() result performs no subscription, while a nonempty
# result reaches the ordinary subscription error.
def update_from(source):
    target = {"keep": 1}
    target.update(source)
    return target


show("12_update_empty", lambda: update_from(KeysEmpty()))
run("13_update_nonempty", lambda: update_from(KeysNoGetitem()))
show("14_display_empty", lambda: {**KeysEmpty()})
run("15_display_nonempty", lambda: {**KeysNoGetitem()})


# Counter distinguishes mapping input from element input more strictly than
# dict expansion: keys() without __getitem__ falls through to iteration even
# when keys() is empty.
run("15a_counter_ctor_keys_empty", lambda: Counter(KeysEmpty()))
run("15b_counter_ctor_keys_nonempty", lambda: Counter(KeysNoGetitem()))
run("15c_counter_update_keys_empty", lambda: Counter().update(KeysEmpty()))
run("15d_counter_subtract_keys_empty", lambda: Counter().subtract(KeysEmpty()))


# Successful expanded calls remain on the success path.
def success(a, b, *, c):
    return a + b + c


show("16_success", lambda: success(*[1, 2], **{"c": 3}))
