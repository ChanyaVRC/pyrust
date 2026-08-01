import sys


direct = 1
del direct
try:
    direct
except NameError as exc:
    print("direct:", type(exc).__name__)
else:
    print("direct: NO ERROR")

through_globals = 2
del globals()["through_globals"]
try:
    through_globals
except NameError as exc:
    print("globals:", type(exc).__name__, str(exc))
else:
    print("globals: NO ERROR")

frame = sys._getframe()
through_frame = 3
del frame.f_locals["through_frame"]
try:
    through_frame
except NameError as exc:
    print("frame:", type(exc).__name__, str(exc))
else:
    print("frame: NO ERROR")

consumed = 4
del globals()["consumed"]
try:
    result = consumed
except NameError as exc:
    print("consumed:", type(exc).__name__)
else:
    print("consumed: NO ERROR", result)

len = "shadowed"
del globals()["len"]
try:
    len
except NameError:
    print("builtin fallback: NameError")
else:
    print("builtin fallback: resolved")

rebound = 5
del globals()["rebound"]
rebound = 6
print("rebound:", rebound)


def run_alias_case(label, source, namespace=None):
    if namespace is None:
        namespace = {}
    try:
        exec(source, namespace)
    except NameError:
        print(label + ": NameError")
    else:
        print(label + ": NO ERROR")


run_alias_case(
    "direct alias",
    "g = globals\nns = g()\nb = 2\ndel ns['b']\nb",
)
run_alias_case(
    "attribute alias",
    "import builtins as bi\ng = bi.globals\nns = g()\nb = 2\ndel ns['b']\nb",
)
run_alias_case(
    "import alias",
    "from builtins import globals as g\nns = g()\nb = 2\ndel ns['b']\nb",
)
run_alias_case(
    "getattr alias",
    "import builtins as bi\ng = getattr(bi, 'globals')\nns = g()\nb = 2\ndel ns['b']\nb",
)
run_alias_case(
    "function globals",
    "def owner(): pass\nns = owner.__globals__\nb = 2\ndel ns['b']\nb",
)
run_alias_case(
    "builtins dict",
    "import builtins as bi\ng = bi.__dict__['globals']\nns = g()\nb = 2\ndel ns['b']\nb",
)


def delete_frame_name(frame, name):
    del frame.f_locals[name]


run_alias_case(
    "sys import alias",
    "from sys import _getframe as get_frame\nb = 2\ndelete_frame_name(get_frame(), 'b')\nb",
    {"delete_frame_name": delete_frame_name},
)
