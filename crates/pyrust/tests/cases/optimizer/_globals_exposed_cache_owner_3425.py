# Helper module for test_globals_exposed_cache_3425.py (#3425).
#
# A filesystem module whose functions read their own module globals through
# cached `LoadGlobal` entries. The fixture exposes this module's dictionary
# through `vars()`, `__dict__`, and `function.__globals__`, mutates it through
# those aliases, and checks that these readers observe every change.

level = 1
label = "owner"


def helper(value):
    return value + level


def read_level():
    return level


def read_label():
    return label


def call_helper(value):
    return helper(value)


def use_len(items):
    return len(items)


def read_missing():
    try:
        return late_binding
    except NameError as error:
        return "NameError: " + str(error)


def set_level(value):
    global level
    level = value


def bump_level(count):
    global level
    for _ in range(count):
        level = level + 1
    return level


def mutate(namespace, name, value):
    namespace[name] = value


def discard(namespace, name):
    return namespace.pop(name, "absent")


def own_globals():
    return globals()
