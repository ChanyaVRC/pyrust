"""CallMemo may cache only inputs and bindings represented by its key."""


# Omitting x makes the result depend on mutable function.__defaults__ state.
def plus_one(x=1):
    return x + 1


print("defaults-before", plus_one())
plus_one.__defaults__ = (5,)
print("defaults-after", plus_one())


# A registry spelling does not prove that the active global binding is the
# canonical builtin.
builtin_calls = 0


def fake_abs(value):
    global builtin_calls
    builtin_calls += 1
    return builtin_calls


abs = fake_abs


def wrapped_builtin(value):
    return abs(value)


print("builtin-binding", wrapped_builtin(5), wrapped_builtin(5), builtin_calls)


# A previously memo-pure sibling name is also a mutable binding. Only direct
# self-recursion is identity-stable enough for the compile-time fixpoint.
sibling_calls = 0


def original(value):
    return value + 10


def wrapped_sibling(value):
    return original(value)


def replacement_sibling(value):
    global sibling_calls
    sibling_calls += 1
    return sibling_calls


original = replacement_sibling
print("sibling-binding", wrapped_sibling(5), wrapped_sibling(5), sibling_calls)
