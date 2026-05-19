# PEP 479 __cause__ chaining: when StopIteration escapes a generator, the
# resulting RuntimeError must carry the original StopIteration as __cause__
# and __suppress_context__ must be True.  CPython 3.12 behaviour.

# --- basic: __cause__ is the original StopIteration with its message ---
def gen_cause_basic():
    yield
    raise StopIteration("original")

g = gen_cause_basic()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("cause repr:", repr(e.__cause__))
    print("cause type:", type(e.__cause__).__name__)
    print("cause str:", str(e.__cause__))
    print("suppress_context:", e.__suppress_context__)
    # CPython also sets __context__ to the same StopIteration instance.
    print("context type:", type(e.__context__).__name__)
    print("context is cause:", e.__context__ is e.__cause__)

# --- StopIteration with no arguments: __cause__ is StopIteration() not None ---
def gen_cause_no_args():
    yield
    raise StopIteration()

g = gen_cause_no_args()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("no-args cause repr:", repr(e.__cause__))
    print("no-args cause type:", type(e.__cause__).__name__)
    print("no-args suppress_context:", e.__suppress_context__)
    print("no-args context type:", type(e.__context__).__name__)

# --- StopIteration subclass: __cause__ is instance of the subclass ---
class MyStop(StopIteration):
    pass

def gen_cause_subclass():
    yield
    raise MyStop("custom")

g = gen_cause_subclass()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("subclass cause type:", type(e.__cause__).__name__)
    print("subclass cause str:", str(e.__cause__))
    print("subclass suppress_context:", e.__suppress_context__)

# --- The wrapped exception is still a RuntimeError ---
def gen_is_runtime_error():
    yield
    raise StopIteration("test")

g = gen_is_runtime_error()
next(g)
try:
    next(g)
except RuntimeError as e:
    print("is RuntimeError:", isinstance(e, RuntimeError))

# --- StopIteration in a regular function: unaffected, no __cause__ set ---
def regular_fn():
    raise StopIteration("not in generator")

try:
    regular_fn()
except StopIteration as e:
    print("regular fn StopIteration:", str(e))
