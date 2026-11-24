import builtins
import os
import sys


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__} {exc.args!r}")


# --- builtins registration & metadata ---
print("is builtins.breakpoint:", breakpoint is builtins.breakpoint)
print("hasattr builtins:", hasattr(builtins, "breakpoint"))
print("in dir builtins:", "breakpoint" in dir(builtins))
print("repr:", repr(breakpoint))
print("name:", breakpoint.__name__)
print("qualname:", breakpoint.__qualname__)
print("module:", breakpoint.__module__)
print("self is builtins:", breakpoint.__self__ is builtins)
print("default hook identity:", sys.breakpointhook is sys.__breakpointhook__)


orig_hook = sys.breakpointhook
orig_env = os.environ.get("PYTHONBREAKPOINT")

try:
    # --- monkey-patching sys.breakpointhook ---
    calls = []

    def custom_hook(*args, **kwargs):
        calls.append((args, sorted(kwargs.items())))
        return ("ret", len(args), len(kwargs))

    sys.breakpointhook = custom_hook
    print("no args:", breakpoint())
    print("pos args:", breakpoint(1, "two", [3]))
    print("pos+kw args:", breakpoint(10, a=1, b=2))
    print("kw only:", breakpoint(flag=True))
    print("calls:", calls)

    # --- callable object & bound method hooks ---
    class CallableHook:
        def __init__(self, tag):
            self.tag = tag

        def __call__(self, *args, **kwargs):
            return (self.tag, args, kwargs)

        def method(self, x, y=0):
            return (self.tag, x + y)

    ch = CallableHook("obj")
    sys.breakpointhook = ch
    print("callable obj:", breakpoint(5, k="v"))

    sys.breakpointhook = ch.method
    print("bound method:", breakpoint(3, y=4))

    # --- hook raising exception propagates ---
    def failing_hook(*args, **kwargs):
        raise ValueError("hook failed")

    sys.breakpointhook = failing_hook
    show("hook raises", lambda: breakpoint(1))

    # --- non-callable sys.breakpointhook ---
    sys.breakpointhook = 123
    show("non-callable int", lambda: breakpoint())
    sys.breakpointhook = None
    show("non-callable None", lambda: breakpoint())

    # --- deleted sys.breakpointhook ---
    del sys.breakpointhook
    show("deleted hook", lambda: breakpoint())

    # --- default sys.__breakpointhook__ with PYTHONBREAKPOINT ---
    sys.breakpointhook = sys.__breakpointhook__
    os.environ["PYTHONBREAKPOINT"] = "0"
    print("PYTHONBREAKPOINT=0:", breakpoint())
    print("PYTHONBREAKPOINT=0 with args:", breakpoint(1, 2, a=3))

    os.environ["PYTHONBREAKPOINT"] = "builtins.len"
    print("PYTHONBREAKPOINT=builtins.len:", breakpoint([10, 20, 30]))

    os.environ["PYTHONBREAKPOINT"] = "builtins.dict"
    print("PYTHONBREAKPOINT=builtins.dict:", breakpoint(x=1, y=2))

    # --- sys.addaudithook integration ---
    audit_events = []

    def on_audit(event, args):
        if event == "builtins.breakpoint":
            audit_events.append((event, len(args), args[0] is sys.breakpointhook))

    sys.addaudithook(on_audit)
    sys.breakpointhook = lambda *a, **kw: "audited"
    print("audited call:", breakpoint())
    print("audit events:", audit_events)
finally:
    sys.breakpointhook = orig_hook
    if orig_env is None:
        if "PYTHONBREAKPOINT" in os.environ:
            del os.environ["PYTHONBREAKPOINT"]
    else:
        os.environ["PYTHONBREAKPOINT"] = orig_env
