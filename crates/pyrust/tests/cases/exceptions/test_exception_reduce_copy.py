import copy
# __reduce__ matrix
for e in [ValueError(), ValueError("m"), ValueError("a", 2), KeyError("k"), OSError(2, "no such")]:
    r = e.__reduce__()
    print(type(e).__name__, r[0].__name__, r[1], len(r))
# attrs set -> state dict appears
e = ValueError("m"); e.extra = 42
r = e.__reduce__()
print(r[0].__name__, r[1], r[2] if len(r) > 2 else None)
# round-trip
cls, args = ValueError("x", 1).__reduce__()[:2]
e2 = cls(*args); print(type(e2).__name__, e2.args)
# subclass with custom __init__
class MyErr(Exception):
    def __init__(self, code): super().__init__(f"code={code}"); self.code = code
m = MyErr(7)
r = m.__reduce__(); print(r[0].__name__, r[1], r[2] if len(r) > 2 else None)
# copy/deepcopy: tb reset, attrs kept, subclass type
def boom(): raise MyErr(9)
try: boom()
except MyErr as e3:
    _ = e3.__traceback__  # materialized case
    c = copy.copy(e3); d = copy.deepcopy(e3)
    print(type(c).__name__, c.args, c.code, c.__traceback__, d.__traceback__)
    print(e3.__traceback__ is not None)
try: boom()
except MyErr as e4:
    c2 = copy.copy(e4)  # never-read case (placeholder safety)
    print(c2.__traceback__, type(c2.__traceback__).__name__)
# chained exceptions under copy
try:
    try: raise ValueError("in")
    except ValueError as iv: raise KeyError("out") from iv
except KeyError as ke:
    dk = copy.deepcopy(ke)
    print(dk.__cause__, type(dk.__cause__).__name__ if dk.__cause__ else None, dk.__suppress_context__)
# deepcopy recurses args
e5 = ValueError([1, [2]])
d5 = copy.deepcopy(e5)
d5.args[0][1][0] = 99
print(e5.args, d5.args)
# notes
e6 = ValueError("n"); e6.add_note("note1")
c6 = copy.copy(e6)
print(getattr(c6, "__notes__", None))
