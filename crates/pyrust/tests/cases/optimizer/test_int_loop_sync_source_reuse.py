# The specialized int-loop copy (ARCHITECTURE rule 29) removes the body's
# `SyncModuleGlobal`s and replays them from an exit stub.  Two facts that
# deferral depends on are pinned here, because a live namespace alias observes
# both of them:
#
# 1. The source register must still hold its published value when the stub runs.
#    A module-scope `name = <expr>` reaches the optimizer as `BinOpConst(t, …)` +
#    `Move(local, t)` + `SyncModuleGlobal(t, …)` over a scratch register `t` that
#    the next expression immediately reuses, so one register is routinely the
#    source of several synced names.  Replaying those from the stub would publish
#    the register's *last* value under every name it ever synced, and on a first
#    entry bind names the original loop never bound at all.
#
# 2. Every real loop exit must reach a stub.  An inverted-while header is the
#    zero-trip test on entry, but a `continue` edge jumps back to it, and then it
#    is also the exhaustion test for every iteration that took that edge.
#
# Nothing below may depend on whether a loop was versioned.

probe = globals()


# ── Two names published from one reused scratch register ─────────────────────
i = 0
tot = 0
while i < 4:
    a = i * 2
    c = i + 1
    tot += a + c
    i += 1
print("two names", a, c, i, tot, probe["a"], probe["c"])


# The same shape with both names already bound, so every entry guard a versioned
# copy would install can pass.
j = 0
tot2 = 0
a2 = 0
c2 = 0
while j < 4:
    a2 = j * 2
    c2 = j + 1
    tot2 += a2 + c2
    j += 1
print("prebound", a2, c2, j, tot2)


# An int left in a scratch register ahead of the loop is what used to let the
# copy run for this shape at all; the published values must not depend on it.
spill = 1 + 1
k = 0
tot3 = 0
while k < 4:
    a3 = k * 2
    c3 = k + 1
    tot3 += a3 + c3
    k += 1
print("after spill", spill, a3, c3, k, tot3)


# ── A mid-loop side exit must not publish a name the original had not bound ──
xs = [object(), 1, 2, 3]
m = 0
tot4 = 0
try:
    while m < 4:
        v = xs[m]
        d = v * 2
        e = v + 1
        tot4 += d + e
        m += 1
except TypeError:
    pass
print("side exit at entry", m, tot4, "d" in probe, "e" in probe, "v" in probe)


# The same subscript loop deopting after two good iterations keeps the values
# those iterations published.
ys = [1, 2, object(), 4]
p = 0
tot5 = 0
try:
    while p < 4:
        w = ys[p]
        f = w * 2
        g = w + 1
        tot5 += f + g
        p += 1
except TypeError:
    pass
print("side exit mid loop", p, tot5, probe.get("f"), probe.get("g"), type(probe.get("w")).__name__)


# A `for` over a sequence whose first element is not an int side-exits before
# the body has bound anything.
tot6 = 0
try:
    for x in [object(), 1, 2]:
        h = x * 2
        n2 = x + 1
        tot6 += h + n2
except TypeError:
    pass
print("for side exit", tot6, "h" in probe, "n2" in probe, "x" in probe)


# ── Every loop exit reaches the deferred syncs ───────────────────────────────
# The last iteration takes the `continue` edge, so the loop leaves through the
# header rather than the back-edge.
q = 0
acc = 0
while q < 7:
    if q % 3 == 0:
        q += 1
        continue
    acc += q
    q += 1
print("continue exit", q, acc, probe["q"], probe["acc"])


# The last iteration falls through to the back-edge instead.
r = 0
acc2 = 0
while r < 6:
    if r % 3 == 0:
        r += 1
        continue
    acc2 += r
    r += 1
print("backedge exit", r, acc2, probe["r"], probe["acc2"])


# A zero-trip entry binds nothing and publishes nothing new.
s = 99
acc3 = 0
while s < 7:
    if s % 3 == 0:
        s += 1
        continue
    acc3 += s
    s += 1
print("zero trip", s, acc3, probe["s"], probe["acc3"])


# The inverted shape the guard retirement targets, read back through the alias.
t = 0
total = 0
while t < 40:
    if t % 2 != 0:
        total += t
    t += 1
print("inverted", t, total, probe["t"], probe["total"])


# A `break` exit publishes what the broken-out-of iteration produced.
u = 0
acc4 = 0
while u < 40:
    acc4 += u
    if acc4 > 10:
        break
    u += 1
print("break exit", u, acc4, probe["u"], probe["acc4"])

print("names bound", sorted(n for n in ("a", "c", "a3", "d", "e", "h", "n2") if n in probe))
