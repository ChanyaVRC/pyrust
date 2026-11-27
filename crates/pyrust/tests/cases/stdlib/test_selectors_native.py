# CPython 3.12.3 parity tests for the native `selectors` module (issue #3453).
#
# Everything printed here is compared byte-for-byte with CPython, so nothing
# platform- or address-dependent is shown: platform selectors (poll, epoll,
# kqueue) are exercised through whichever of them the platform provides, and
# every file object that is printed has an explicit `__repr__`.

import io
import selectors
import socket
import sys
import time
from collections import namedtuple
from collections.abc import Mapping

R = selectors.EVENT_READ
W = selectors.EVENT_WRITE


def run(fn, *args, **kwargs):
    try:
        r = fn(*args, **kwargs)
        return ("ok", ascii(r))
    except BaseException as e:
        return (type(e).__name__, str(e)[:120])


def err(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except BaseException as e:
        return type(e).__name__, str(e)
    return None


class Obj:
    """File-like object with a configurable `fileno()`."""

    def __init__(self, name, fd):
        self.name = name
        self.fd = fd

    def fileno(self):
        if isinstance(self.fd, BaseException):
            raise self.fd
        if callable(self.fd):
            return self.fd()
        return self.fd

    def __repr__(self):
        return "Obj(%s)" % self.name


# 1. Module surface ------------------------------------------------------------
print("meta:", selectors.__name__, ascii(selectors.__doc__))
print("events:", selectors.EVENT_READ, selectors.EVENT_WRITE)
print("modules:", [
    c.__module__
    for c in (
        selectors.SelectorKey,
        selectors.BaseSelector,
        selectors.SelectSelector,
        selectors._BaseSelectorImpl,
        selectors._SelectorMapping,
    )
])
print("imports:", selectors.select.__name__, selectors.sys is sys, selectors.Mapping is Mapping,
      selectors.namedtuple is namedtuple, selectors.math.__name__)
print("public:", sorted(
    n for n in ("EVENT_READ", "EVENT_WRITE", "SelectorKey", "BaseSelector", "SelectSelector",
                "DefaultSelector", "_fileobj_to_fd", "_SelectorMapping", "_BaseSelectorImpl",
                "_PollLikeSelector", "_can_use", "PollSelector", "EpollSelector",
                "KqueueSelector", "DevpollSelector")
    if hasattr(selectors, n)
))
print("has __all__:", hasattr(selectors, "__all__"))
print("mro:", [c.__name__ for c in selectors.SelectSelector.__mro__])
print("mro default:", [c.__name__ for c in selectors.DefaultSelector.__mro__][1:])
print("mapping mro:", [c.__name__ for c in selectors._SelectorMapping.__mro__][:3])
print("issubclass:", issubclass(selectors.SelectSelector, selectors.BaseSelector),
      issubclass(selectors.DefaultSelector, selectors.BaseSelector),
      issubclass(selectors._SelectorMapping, Mapping))
print("metaclass:", type(selectors.BaseSelector).__name__, type(selectors.SelectSelector).__name__)
print("docs:", ascii(selectors.BaseSelector.__doc__[:30]), ascii(selectors.SelectSelector.__doc__),
      ascii(selectors._BaseSelectorImpl.__doc__), ascii(selectors._SelectorMapping.__doc__))
print("abstract:", sorted(selectors.BaseSelector.__abstractmethods__),
      sorted(selectors._BaseSelectorImpl.__abstractmethods__),
      sorted(selectors.SelectSelector.__abstractmethods__),
      sorted(selectors.DefaultSelector.__abstractmethods__))
print("instantiate base:", err(selectors.BaseSelector))
print("instantiate impl:", err(selectors._BaseSelectorImpl))
print("default is concrete:", type(selectors.DefaultSelector()).__name__ == selectors.DefaultSelector.__name__)

# 2. SelectorKey ---------------------------------------------------------------
K = selectors.SelectorKey
print("key fields:", K._fields, K.__name__, K.__mro__[1].__name__, K._field_defaults)
print("key doc:", ascii(K.__doc__))
k = K("f", 3, R, {"a": 1})
print("key tuple:", isinstance(k, tuple), len(k), k[0], k[1], k[2], k[3], k[-1], k[1:3])
a, b, c, d = k
print("key unpack:", a, b, c, d)
print("key attrs:", k.fileobj, k.fd, k.events, k.data)
print("key repr:", repr(k))
print("key asdict:", k._asdict())
print("key replace:", k._replace(data=None), k._replace(events=W, fd=9))
print("key eq/hash:", k == ("f", 3, R, {"a": 1}), k == K("f", 3, R, {"a": 2}),
      hash(K("f", 3, R, None)) == hash(("f", 3, R, None)))
print("key errors:", err(K, 1, 2)[0], err(K, 1, 2, 3, 4, 5)[0], err(setattr, k, "fd", 1)[0])
print("key kw:", K(fileobj=1, fd=2, events=R, data=4), K(1, 2, R, data=None))
print("key make:", K._make([1, 2, 3, 4]), tuple(k), list(k), k.count(3), k.index(3))
print("key sort:", sorted([K("b", 2, R, None), K("a", 1, R, None)], key=lambda x: x.fd)[0].fileobj)

# 3. _fileobj_to_fd ------------------------------------------------------------
to_fd = selectors._fileobj_to_fd
print("fd int:", run(to_fd, 5), run(to_fd, 0), run(to_fd, True), run(to_fd, False))
print("fd negative:", run(to_fd, -1), run(to_fd, -7))
print("fd obj:", run(to_fd, Obj("a", 12)), run(to_fd, Obj("neg", -1)), run(to_fd, Obj("neg2", -9)))
print("fd float:", run(to_fd, Obj("float", 3.7)), run(to_fd, Obj("str", "4")), run(to_fd, Obj("strbad", "x")))
print("fd none:", run(to_fd, None), run(to_fd, "abc"), run(to_fd, 1.5), run(to_fd, object()) [0])
print("fd raises:",
      run(to_fd, Obj("v", ValueError("closed"))),
      run(to_fd, Obj("t", TypeError("tt"))),
      run(to_fd, Obj("a", AttributeError("aa"))),
      run(to_fd, Obj("u", io.UnsupportedOperation("fileno"))))
print("fd propagates:", run(to_fd, Obj("o", OSError("os"))), run(to_fd, Obj("k", KeyError("kk"))),
      run(to_fd, Obj("r", RuntimeError("rr"))))
print("fd cause:", end=" ")
try:
    to_fd(Obj("v", ValueError("closed")))
except ValueError as e:
    print(e.__cause__, e.__suppress_context__)
print("fd huge:", run(to_fd, 2 ** 40), run(to_fd, Obj("huge", lambda: 2 ** 70)) [0])


class IntSub(int):
    pass


print("fd int subclass:", run(to_fd, IntSub(8)), type(to_fd(IntSub(8))).__name__,
      type(to_fd(True)).__name__)

# 4. Registration bookkeeping ---------------------------------------------------
sel = selectors.SelectSelector()
print("fresh:", sel.get_map() is not None, len(sel.get_map()), list(sel.get_map()), sorted(vars(sel)))
o1, o2, o3 = Obj("o1", 11), Obj("o2", 12), Obj("o3", 13)
k1 = sel.register(o1, R, "d1")
k2 = sel.register(12, W)
k3 = sel.register(o3, R | W, data=[1, 2])
print("register:", k1, k2, k3, sep="\n  ")
print("types:", type(k1).__name__, k1.fileobj is o1, k2.fileobj, k3.data)
print("vars:", sorted(vars(sel)), sorted(sel._readers), sorted(sel._writers), sorted(sel._fd_to_key))
print("get_key:", sel.get_key(o1) is k1, sel.get_key(11) is k1, sel.get_key(12) is k2, sel.get_key(o3) is k3)
print("get_key missing:", err(sel.get_key, 99), err(sel.get_key, o2) is None, err(sel.get_key, -1),
      err(sel.get_key, Obj("nofd", ValueError("x"))))
m = sel.get_map()
print("map type:", type(m).__name__, isinstance(m, Mapping), sorted(vars(m)), m is sel.get_map(), m is sel._map)
print("map basics:", len(m), sorted(m), 11 in m, o1 in m, 99 in m, Obj("x", 99) in m, run(m.__contains__, None), run(m.__contains__, -1))
print("map getitem:", m[o1] is k1, m[11] is k1, m[12] is k2, m.get(13) is k3, m.get(99), m.get(99, "dflt"))
print("map keyerror:", err(m.__getitem__, 99), err(m.__getitem__, o2) is None, err(m.__getitem__, -1),
      err(m.__getitem__, None))
print("map views:", sorted(m.keys()), [v.fd for v in m.values()], sorted((k, v.fd) for k, v in m.items()))
print("map order:", list(m), list(m.keys()), [kk.fd for kk in m.values()])
print("map eq:", m == m, m == dict(m.items()), m != {}, dict(m) == dict(m.items()))
print("map readonly:", hasattr(m, "__setitem__"), hasattr(m, "__delitem__"), hasattr(m, "pop"), hasattr(m, "update"))
try:
    m[1] = 2
except TypeError as e:
    print("map setitem:", e)
print("map unhashable:", err(hash, m))
print("map live:", end=" ")
o4 = Obj("o4", 14)
sel.register(o4, R)
print(len(m), 14 in m, o4 in m, m[o4].fd)
sel.unregister(14)
print("map live2:", len(m), 14 in m, o4 in m)
print("map reflects modify:", end=" ")
sel.modify(13, W, "new")
print(m[13].events, m[13].data, m[o3].data)

# 5. register errors ------------------------------------------------------------
print("dup:", err(sel.register, o1, R), err(sel.register, 11, W), err(sel.register, 12, R))
print("invalid events:", err(sel.register, 20, 0), err(sel.register, 20, 4), err(sel.register, 20, 3 | 4),
      err(sel.register, 20, -1), err(sel.register, 20, None) )
print("invalid events types:", err(sel.register, 20, "r"), err(sel.register, 20, 1.5)[0] if err(sel.register, 20, 1.5) else None)
print("events order:", err(sel.register, -5, 0), err(sel.register, -5, R), err(sel.register, o2, 0))
print("invalid fileobj:", err(sel.register, None, R), err(sel.register, "x", R), err(sel.register, object(), R)[0],
      err(sel.register, -3, R), err(sel.register, Obj("n", -4), R))
print("bool events:", run(sel.register, 21, True), sorted(sel._fd_to_key))
print("after errors:", sorted(sel._fd_to_key), sorted(sel._readers), sorted(sel._writers))
print("data default:", sel.register(22, R).data, sel.register(23, events=R, data="kw").data)

# 6. unregister ----------------------------------------------------------------
print("unregister:", sel.unregister(o1) is k1, sorted(sel._fd_to_key), sorted(sel._readers))
print("unregister missing:", err(sel.unregister, o1), err(sel.unregister, 99), err(sel.unregister, -2),
      err(sel.unregister, None))
print("unregister by fd:", sel.unregister(12) is k2, sorted(sel._writers))
print("unregister again:", err(sel.unregister, 12))
sel.close()

# closed file objects ---------------------------------------------------------
sel = selectors.SelectSelector()
closed = Obj("closed", 31)
sel.register(closed, R, "c")
closed.fd = ValueError("I/O operation on closed file")
print("closed lookup:", sel._fileobj_lookup(closed), err(selectors._fileobj_to_fd, closed))
print("closed get_key:", sel.get_key(closed).data, sel.get_map()[closed].fd, closed in sel.get_map())
print("closed modify:", sel.modify(closed, R, "c2").data, sel.get_key(closed).data)
print("closed unregister:", sel.unregister(closed).fd, list(sel.get_map()), sorted(sel._readers))
neg = Obj("neg", 32)
sel.register(neg, W)
neg.fd = -1
print("neg lookup:", err(selectors._fileobj_to_fd, neg), sel._fileobj_lookup(neg), neg in sel.get_map())
print("neg unregister:", sel.unregister(neg).fd, len(sel.get_map()))
other = Obj("other", ValueError("never registered"))
print("closed never registered:", err(sel.unregister, other), err(sel.register, other, R), err(sel.get_key, other))
sel.close()

# identity-based fallback must use `is`, not equality
class Eq:
    def __init__(self, fd):
        self.fd = fd

    def __eq__(self, other):
        return True

    def __hash__(self):
        return 1

    def fileno(self):
        if self.fd is None:
            raise ValueError("closed")
        return self.fd

    def __repr__(self):
        return "Eq"


sel = selectors.SelectSelector()
e1, e2 = Eq(41), Eq(42)
sel.register(e1, R)
sel.register(e2, R)
e1.fd = None
print("identity:", sel._fileobj_lookup(e1), err(sel._fileobj_lookup, Eq(None)), sel.unregister(e1).fd, sorted(sel._fd_to_key))
sel.close()

# 7. modify --------------------------------------------------------------------
sel = selectors.SelectSelector()
base = sel.register(51, R, "a")
print("modify data only:", end=" ")
same = sel.modify(51, R, "a")
print(same is base, sel.get_key(51) is base)
newd = sel.modify(51, R, "b")
print(newd is base, newd.data, newd.events, sel.get_key(51) is newd, sel.get_map()[51] is newd)
newe = sel.modify(51, W, "b")
print("modify events:", newe.events, newe.data, sorted(sel._readers), sorted(sel._writers), list(sel.get_map()))
newboth = sel.modify(51, R | W)
print("modify both:", newboth.events, newboth.data, sorted(sel._readers), sorted(sel._writers))
print("modify eq data:", sel.modify(51, R | W, None) is newboth)
print("modify errors:", err(sel.modify, 99, R), err(sel.modify, -1, R), err(sel.modify, None, R),
      err(sel.modify, 51, 0), err(sel.modify, 51, 8))
print("after bad modify:", sorted(sel._fd_to_key), sorted(sel._readers), sorted(sel._writers))
sel.register(51, R | W)
sel.register(52, R, "x")
sel.register(53, R, "y")
sel.modify(51, W, "moved")
print("order kept:", list(sel.get_map()))
print("modify returns key:", type(sel.modify(52, W)).__name__)
sel.close()

# 8. close / context manager ----------------------------------------------------
sel = selectors.SelectSelector()
sel.register(61, R)
m = sel.get_map()
print("before close:", len(m), sel.get_map() is m)
print("close:", sel.close(), sel.get_map(), sel._map, sel._fd_to_key, sorted(sel._readers))
print("after close:", len(m), list(m), 61 in m, err(m.__getitem__, 61))
print("get_key closed:", err(sel.get_key, 61), err(sel.get_key, None), err(sel.get_key, -1))
print("close twice:", sel.close(), sel.get_map())
print("register closed:", err(sel.register, 62, R), sorted(sel._fd_to_key))
print("unregister closed:", err(sel.unregister, 62) is None, err(sel.unregister, 62))

with selectors.SelectSelector() as ctx:
    print("with:", type(ctx).__name__, ctx.get_map() is not None)
    ctx.register(63, R)
print("with exit:", ctx.get_map(), ctx._fd_to_key)
try:
    with selectors.SelectSelector() as ctx2:
        raise KeyError("boom")
except KeyError as e:
    print("with error:", e, ctx2.get_map())
print("enter/exit:", selectors.BaseSelector.__enter__.__name__, selectors.BaseSelector.__exit__.__name__)
ctx3 = selectors.SelectSelector()
print("manual exit:", ctx3.__enter__() is ctx3, ctx3.__exit__(None, None, None), ctx3.get_map())

# 9. select() on loopback sockets ------------------------------------------------
srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.bind(("127.0.0.1", 0))
srv.listen(5)
addr = srv.getsockname()
cli = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
cli.connect(addr)
conn, _ = srv.accept()
idle = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
idle.bind(("127.0.0.1", 0))


def fmt(ready):
    return sorted((k.data, ev) for k, ev in ready)


def run_select_suite(factory, label):
    out = []
    with factory() as sel:
        if sys.platform != "win32":
            # Winsock rejects `select()` with three empty sets (WSAEINVAL).
            out.append(("empty", sel.select(0), sel.select(0.01)))
        sel.register(srv, R, "srv")
        sel.register(conn, R, "conn")
        sel.register(cli, W, "cli")
        out.append(("types", type(sel.select(0)).__name__, type(sel.select(0)[0]).__name__))
        r = sel.select(0)
        out.append(("timeout0", fmt(r), all(type(k) is selectors.SelectorKey for k, _ in r)))
        r = sel.select(timeout=0.05)
        out.append(("write ready", fmt(r)))
        out.append(("negative timeout", fmt(sel.select(-5)), fmt(sel.select(-0.5))))
        out.append(("int timeout", fmt(sel.select(1)), fmt(sel.select(True))))
        sel.modify(cli, R | W, "cli")
        out.append(("rw cli", fmt(sel.select(0))))
        cli.sendall(b"ping")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            r = sel.select(0.1)
            if any(k.data == "conn" for k, _ in r):
                break
        out.append(("read ready", fmt(r)))
        out.append(("recv", conn.recv(16)))
        out.append(("drained", fmt(sel.select(0.02))))
        sel.modify(cli, R, "cli")
        sel.unregister(cli)
        out.append(("only conn+srv", fmt(sel.select(0.02))))
        sel.modify(conn, W, "connw")
        out.append(("conn writable", fmt(sel.select(0))))
        sel.modify(conn, R | W, "connrw")
        cli2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        cli2.connect(addr)
        t0 = time.monotonic()
        deadline = t0 + 5
        while time.monotonic() < deadline:
            r = sel.select(0.1)
            if any(k.data == "srv" for k, _ in r):
                break
        out.append(("listen ready", fmt(r)))
        acc, _ = srv.accept()
        acc.close()
        cli2.close()
        sel.unregister(srv)
        sel.unregister(conn)
        sel.register(idle, R, "idle")
        t0 = time.monotonic()
        r = sel.select(0.2)
        elapsed = time.monotonic() - t0
        out.append(("idle wait", r, 0.15 <= elapsed < 3.0))
        out.append(("idle fraction", sel.select(0.001), sel.select(1e-9)))
        sel.unregister(idle)
    print(label)
    for item in out:
        print("   ", item)


run_select_suite(selectors.SelectSelector, "select suite (SelectSelector):")
run_select_suite(selectors.DefaultSelector, "select suite (DefaultSelector):")
for name in ("PollSelector", "EpollSelector", "KqueueSelector"):
    if hasattr(selectors, name):
        run_select_suite(getattr(selectors, name), "select suite (%s):" % name)

# events mask is limited to the registered ones ---------------------------------
with selectors.DefaultSelector() as sel:
    sel.register(cli, R, "r-only")
    print("mask:", fmt(sel.select(0)))
    cli.close()
    conn.close()
with selectors.SelectSelector() as sel:
    sel.register(idle, R, "idle")
    t0 = time.monotonic()
    print("idle select timeout:", sel.select(0.05), time.monotonic() - t0 < 3.0)
srv.close()
idle.close()

# 10. UDP, pipes of fds -----------------------------------------------------------
udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
udp.bind(("127.0.0.1", 0))
snd = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
with selectors.DefaultSelector() as sel:
    sel.register(udp, R, "udp")
    print("udp idle:", sel.select(0.02))
    snd.sendto(b"dgram", udp.getsockname())
    deadline = time.monotonic() + 5
    ready = []
    while not ready and time.monotonic() < deadline:
        ready = sel.select(0.1)
    print("udp ready:", [(k.data, ev) for k, ev in ready], udp.recvfrom(16)[0])
    print("fd register:", sel.register(snd.fileno(), W, "fd").fd == snd.fileno())
    print("fd select:", fmt(sel.select(0.05)))
udp.close()
snd.close()

# 11. Subclassing: super(), `_select` hook, abstract enforcement -----------------
class Tracking(selectors.SelectSelector):
    def __init__(self):
        super().__init__()
        self.log = []

    def register(self, fileobj, events, data=None):
        self.log.append(("register", events))
        return super().register(fileobj, events, data)

    def unregister(self, fileobj):
        self.log.append(("unregister",))
        return super().unregister(fileobj)

    def modify(self, fileobj, events, data=None):
        self.log.append(("modify", events))
        return super().modify(fileobj, events, data)

    def close(self):
        self.log.append(("close",))
        super().close()


t = Tracking()
t.register(71, R)
t.modify(71, W, "x")
t.modify(71, W, "y")
t.unregister(71)
t.close()
print("tracking:", t.log, t.get_map())


class Fake(selectors.SelectSelector):
    calls = []

    def _select(self, r, w, x, timeout=None):
        Fake.calls.append((sorted(r), sorted(w), list(x), timeout))
        return [81, 82], [82, 83, 99], []


f = Fake()
f.register(81, R, "a")
f.register(82, R | W, "b")
f.register(83, R, "c")
res = f.select(None)
print("fake select:", sorted((k.data, ev) for k, ev in res), Fake.calls)
f.select(2)
f.select(-3)
f.select(0.5)
f.select(0)
print("fake timeouts:", [c[3] for c in Fake.calls], [type(c[3]).__name__ for c in Fake.calls])
print("fake masks:", sorted((k.data, ev) for k, ev in f.select(0)))


class Interrupted(selectors.SelectSelector):
    def _select(self, r, w, x, timeout=None):
        raise InterruptedError("EINTR")


i = Interrupted()
i.register(91, R)
print("interrupted:", i.select(0), i.select())


class Failing(selectors.SelectSelector):
    def _select(self, r, w, x, timeout=None):
        raise OSError(9, "bad descriptor")


fl = Failing()
fl.register(92, R)
print("select oserror:", err(fl.select, 0))


class Partial(selectors.BaseSelector):
    def register(self, fileobj, events, data=None):
        return "r"

    def unregister(self, fileobj):
        return "u"

    def select(self, timeout=None):
        return []

    def get_map(self):
        return {5: "five"}


p = Partial()
print("partial base:", p.register(1, R), p.modify(2, R), p.close(), err(p.get_key, 3), p.get_key(5))


class NoSelect(selectors._BaseSelectorImpl):
    pass


print("no select:", err(NoSelect))


class WithSelect(selectors._BaseSelectorImpl):
    def select(self, timeout=None):
        return [(k, k.events) for k in self._fd_to_key.values()]


ws = WithSelect()
ws.register(5, R | W, "five")
print("impl subclass:", [(k.data, e) for k, e in ws.select()], ws.modify(5, R, "z").events, ws.get_key(5).data)
ws.close()
print("impl close:", ws.get_map(), ws._fd_to_key)
print("base map type:", type(selectors.SelectSelector().get_map()).__name__)

# 12. Platform selectors through the same API ------------------------------------
real = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
FD = real.fileno()
for name in ("PollSelector", "EpollSelector", "KqueueSelector"):
    cls = getattr(selectors, name, None)
    if cls is None:
        continue
    print(name, [c.__name__ for c in cls.__mro__][1:4], issubclass(cls, selectors.BaseSelector),
          hasattr(cls, "fileno"), cls.__doc__)
    s = cls()
    k = s.register(FD, R, "d")
    print(" ", name, k.fd == FD, k.events, sorted(vars(s))[:1] and "vars-ok")
    print(" ", name, "dup:", err(s.register, FD, R)[0], "bad events:", err(s.register, FD + 1, 0))
    print(" ", name, "modify:", s.modify(FD, W, "e").events, s.modify(FD, W, "f").data, s.get_key(FD).data)
    print(" ", name, "unregister:", s.unregister(FD).data, err(s.unregister, FD)[0], len(s.get_map()))
    s.close()
    print(" ", name, "closed:", s.get_map(), err(s.get_key, FD))
    if hasattr(s, "fileno"):
        s2 = cls()
        print(" ", name, "fileno:", isinstance(s2.fileno(), int))
        s2.close()
real.close()

# 13. _can_use / DefaultSelector --------------------------------------------------
print("can_use:", selectors._can_use("nope"), selectors._can_use("devpoll"))
order = [n for n in ("KqueueSelector", "EpollSelector", "DevpollSelector", "PollSelector") if hasattr(selectors, n)]
print("default is best:", selectors.DefaultSelector is (getattr(selectors, order[0]) if order else selectors.SelectSelector))
