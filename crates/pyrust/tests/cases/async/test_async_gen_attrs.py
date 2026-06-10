# Issue #2302: async generators expose ag_* introspection attributes
# (ag_frame / ag_running / ag_code / ag_await), mirroring a coroutine's cr_*
# and a plain generator's gi_*.  Each kind exposes ONLY its own prefix:
#   - async generator -> ag_*  (no gi_*, no cr_*)
#   - coroutine       -> cr_*  (no gi_*, no ag_*)
#   - plain generator -> gi_*  (no ag_*, no cr_*)
# __name__ / __qualname__ apply to all three.


async def agen():
    yield 1


async def coro():
    return 1


def gen():
    yield 1


a = agen()
c = coro()
g = gen()

# --- async generator: ag_* present, gi_* / cr_* absent --------------------
print("ag_running", a.ag_running)
print("ag_frame truthy", a.ag_frame is not None)
print("ag has ag_code", hasattr(a, "ag_code"))
print("ag_await", a.ag_await)
print("ag has gi_running", hasattr(a, "gi_running"))
print("ag has gi_frame", hasattr(a, "gi_frame"))
print("ag has gi_yieldfrom", hasattr(a, "gi_yieldfrom"))
print("ag has cr_running", hasattr(a, "cr_running"))
print("ag has ag_yieldfrom", hasattr(a, "ag_yieldfrom"))
print("ag name", a.__name__, a.__qualname__)

# --- coroutine: cr_* present, gi_* / ag_* absent --------------------------
print("cr_running", c.cr_running)
print("cr_frame truthy", c.cr_frame is not None)
print("cr has cr_code", hasattr(c, "cr_code"))
print("cr_await", c.cr_await)
print("cr has gi_running", hasattr(c, "gi_running"))
print("cr has ag_running", hasattr(c, "ag_running"))
print("cr name", c.__name__, c.__qualname__)

# --- plain generator: gi_* intact, ag_* / cr_* absent ---------------------
print("gi_running", g.gi_running)
print("gi_frame truthy", g.gi_frame is not None)
print("gen has gi_code", hasattr(g, "gi_code"))
print("gi_yieldfrom", g.gi_yieldfrom)
print("gen has ag_running", hasattr(g, "ag_running"))
print("gen has ag_await", hasattr(g, "ag_await"))
print("gen has cr_running", hasattr(g, "cr_running"))
print("gen has gi_await", hasattr(g, "gi_await"))
print("gen name", g.__name__, g.__qualname__)

# --- dir() advertises the kind-specific introspection attrs ---------------
# (CPython 3.11+ also exposes ag_suspended / cr_suspended / cr_origin /
# gi_suspended, which pyrust does not implement; assert only the four
# attributes in scope for #2302 so the fixture stays version-stable.)
da = set(dir(a))
print("ag dir core", sorted(da & {"ag_await", "ag_code", "ag_frame", "ag_running"}))
print("ag dir no gi_*", not any(n.startswith("gi_") for n in da))
dc = set(dir(c))
print("cr dir core", sorted(dc & {"cr_await", "cr_code", "cr_frame", "cr_running"}))
print("cr dir no gi_*", not any(n.startswith("gi_") for n in dc))
print("cr dir no ag_*", not any(n.startswith("ag_") for n in dc))
dg = set(dir(g))
print("gen dir core", sorted(dg & {"gi_code", "gi_frame", "gi_running", "gi_yieldfrom"}))
print("gen dir no ag_*", not any(n.startswith("ag_") for n in dg))
print("gen dir no cr_*", not any(n.startswith("cr_") for n in dg))

c.close()
