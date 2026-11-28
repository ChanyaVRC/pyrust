# random / _random — native Rust port parity (issue #3494).
#
# Pins the exact CPython 3.12.3 streams for every seed type (int, negative
# int, big int, float, str, bytes, bytearray, version=1), every generator
# method, the getstate()/setstate()/pickle round trips, SystemRandom, the
# subclass dispatch through self.random() / self.getrandbits(), and the
# error wording.
import copy
import pickle
import random
import _random
from fractions import Fraction


def err(fn):
    try:
        res = fn()
        return "ok %r" % (res,)
    except Exception as e:
        return "%s: %s" % (type(e).__name__, e)


# --- 1. module metadata -------------------------------------------------------
print("__all__", random.__all__)
print("public", sorted(n for n in vars(random) if not n.startswith("_")))
print("consts", random.NV_MAGICCONST.hex(), random.LOG4.hex(), random.SG_MAGICCONST.hex())
print("consts", random.TWOPI, random.BPF, random.RECIP_BPF, random._ONE)
print("VERSION", random.Random.VERSION)
print("mro", [c.__module__ + "." + c.__name__ for c in random.Random.__mro__])
print("sys mro", [c.__name__ for c in random.SystemRandom.__mro__])
print("bases", random.Random.__bases__ == (_random.Random,))
print("_random", [n for n in dir(_random) if not n.startswith("__")], _random.__doc__)
print("_random.Random doc", _random.Random.__doc__)
print("class doc", random.Random.__doc__.splitlines()[0])
print("sys doc", random.SystemRandom.__doc__.splitlines()[0])
print("module doc", random.__doc__.splitlines()[0])
print("Random vars", [n for n in vars(random.Random) if n not in ("__dict__", "__weakref__")])
print("SystemRandom vars", list(vars(random.SystemRandom)))
print("_random vars", [n for n in vars(_random.Random) if n != "__new__"])
print("randbelow alias", random.Random._randbelow is random.Random._randbelow_with_getrandbits)
print("sys randbelow", random.SystemRandom._randbelow is random.Random._randbelow_with_getrandbits)
print("sys state alias", random.SystemRandom.getstate is random.SystemRandom.setstate)
print("_inst", type(random._inst).__name__, random._inst.__dict__)
print("random._random is _random", random._random is _random)
print("module attrs", random.__name__, random.Random.__module__, random.SystemRandom.__module__,
      _random.Random.__module__, random.Random.__qualname__)


# --- 2. seeding: every supported seed type ---------------------------------
SEEDS = [
    0, 1, 42, -42, 2**31, 2**32, 2**32 - 1, 2**64 + 5, -(2**100), 10**50,
    0.5, -0.5, 3.141592653589793, 1e300, -1e-300, float("inf"), 2.0, True, False,
    "", "a", "hello", "héllo ☃ \U0001f600", b"", b"bytes", b"\x00\xff" * 40,
    bytearray(b""), bytearray(b"bytearray"), None,
]
for s in SEEDS:
    if s is None:
        continue
    r = random.Random(s)
    print("seed", repr(s)[:40], [r.random() for _ in range(3)], r.getrandbits(40), r.randrange(10**12))

for s in ["", "a", "hello", "héllo ☃", b"", b"bytes", b"\xff\x00"]:
    r = random.Random()
    r.seed(s, version=1)
    print("seed v1", repr(s), r.random(), r.randrange(1000))

for s in [5, "five", b"five", bytearray(b"five"), 5.5]:
    random.seed(s)
    a = random.random()
    r = random.Random(s)
    print("module seed", repr(s), a == r.random(), a)

# version=3 (anything but 1 / 2) passes str / bytes straight to the base
# seed, which hashes them; ints and floats are unaffected.
r = random.Random()
r.seed(12345, version=3)
print("seed v3 int", r.random() == random.Random(12345).random())
print("seed kw", random.Random(x=99).random() == random.Random(99).random())
r = random.Random(1)
r.seed(a=7)
print("seed a kw", r.random() == random.Random(7).random())

# None seeds from entropy: two generators differ, and the state is complete.
r1, r2 = random.Random(), random.Random(None)
print("entropy", r1.getstate() != r2.getstate(), len(r1.getstate()[1]))

# int subclasses seed by value; float subclasses through hash().
class MyInt(int):
    pass

class MyFloat(float):
    pass

print("int subclass", random.Random(MyInt(77)).random() == random.Random(77).random())
print("float subclass", random.Random(MyFloat(0.25)).random() == random.Random(0.25).random())
print("bool seed", random.Random(True).random() == random.Random(1).random())


# --- 3. getrandbits / randbytes / random ------------------------------------
r = random.Random(2024)
print("getrandbits", [r.getrandbits(k) for k in range(0, 130)])
print("getrandbits big", hex(r.getrandbits(1000)))
r = random.Random(7)
print("randbytes", [r.randbytes(n).hex() for n in range(0, 33)])
print("randbytes 1000", r.randbytes(1000).hex()[-40:])
r = random.Random(3)
print("random", [r.random() for _ in range(10)])
print("module getrandbits", random.Random(9).getrandbits(64) == (random.seed(9) or random.getrandbits(64)))


# --- 4. integer methods ------------------------------------------------------
r = random.Random(11)
print("randrange stop", [r.randrange(10) for _ in range(20)])
print("randrange start stop", [r.randrange(5, 15) for _ in range(10)])
print("randrange step", [r.randrange(0, 100, 7) for _ in range(10)])
print("randrange neg step", [r.randrange(100, 0, -3) for _ in range(10)])
print("randrange negative", [r.randrange(-50, 50) for _ in range(10)])
print("randrange big", r.randrange(2**70), r.randrange(10**30, 10**30 + 10**20, 3))
print("randrange huge", r.randrange(-(10**40), 10**40))
print("randrange kw", r.randrange(start=1, stop=100, step=2), r.randrange(10, step=1))
print("randint", [r.randint(1, 6) for _ in range(20)], r.randint(-10**20, 10**20), r.randint(5, 5))
print("randrange index", r.randrange(True, 10), type(r.randrange(MyInt(3), MyInt(9))).__name__)


# --- 5. sequence methods -----------------------------------------------------
r = random.Random(12)
print("choice", [r.choice([10, 20, 30, 40]) for _ in range(10)])
print("choice types", r.choice((1, 2, 3)), r.choice("abcdef"), r.choice(range(100, 200)), r.choice(b"xyz"))
print("choice dict", r.choice({0: "zero", 1: "one"}))

r = random.Random(13)
print("choices plain", r.choices("abcde", k=10))
print("choices weights", r.choices(["a", "b", "c"], [5, 1, 1], k=10))
print("choices float weights", r.choices(["a", "b", "c"], [0.5, 0.25, 0.25], k=10))
print("choices mixed weights", r.choices(range(4), [1, 2.5, True, 0], k=10))
print("choices cum", r.choices(["a", "b", "c"], cum_weights=[1, 5, 6], k=10))
print("choices cum tuple", r.choices(["a", "b", "c"], cum_weights=(0.1, 0.2, 1.0), k=10))
print("choices fraction", r.choices(["a", "b"], [Fraction(1, 3), Fraction(2, 3)], k=10))
print("choices k0", r.choices([1, 2], k=0), r.choices([1, 2], k=-3), r.choices([1, 2]))

r = random.Random(14)
x = list(range(20))
r.shuffle(x)
print("shuffle", x)
x = ["a", "b", "c", "d", "e"]
r.shuffle(x)
print("shuffle str", x)
r.shuffle([])
r.shuffle([1])

class Seq:
    def __init__(self, data):
        self.data = list(data)
    def __len__(self):
        return len(self.data)
    def __getitem__(self, i):
        return self.data[i]
    def __setitem__(self, i, v):
        self.data[i] = v

s = Seq(range(10))
r.shuffle(s)
print("shuffle custom", s.data)

r = random.Random(15)
print("sample pool", r.sample(range(30), 10))
print("sample set", r.sample(range(10**6), 10))
print("sample set k=6", r.sample(range(200), 6))
print("sample all", r.sample([1, 2, 3, 4, 5], 5))
print("sample tuple", r.sample((1, 2, 3, 4), 2), r.sample("abcdefgh", 3))
print("sample counts", r.sample(["red", "blue"], counts=[4, 2], k=5))
print("sample counts big", r.sample(["x", "y", "z"], counts=[10**6, 1, 10**6], k=8))
print("sample zero", r.sample([1, 2, 3], 0))


# --- 6. binomialvariate ------------------------------------------------------
r = random.Random(16)
print("binom n=1", [r.binomialvariate(1, 0.3) for _ in range(10)], type(r.binomialvariate()).__name__)
print("binom BG", [r.binomialvariate(20, 0.2) for _ in range(10)])
print("binom BTRS", [r.binomialvariate(100, 0.3) for _ in range(10)])
print("binom BTRS large", [r.binomialvariate(10**6, 0.4) for _ in range(5)])
print("binom p>0.5", [r.binomialvariate(100, 0.8) for _ in range(5)], r.binomialvariate(15, 0.6))
print("binom edges", r.binomialvariate(10, 0.0), r.binomialvariate(10, 1.0), r.binomialvariate(0, 0.5),
      r.binomialvariate(5.5, 1.0), r.binomialvariate(7, 0))
print("binom tiny p", r.binomialvariate(1000, 1e-300), r.binomialvariate(10**9, 1e-9))
print("binom float n", r.binomialvariate(20.0, 0.3), r.binomialvariate(200.0, 0.3))
print("binom default", r.binomialvariate())


# --- 7. continuous distributions ---------------------------------------------
r = random.Random(17)
print("uniform", r.uniform(1, 10), r.uniform(-1.5, 2.5), r.uniform(5, 5), r.uniform(10, 1))
print("triangular", r.triangular(), r.triangular(0, 10, 3), r.triangular(0, 10, 9.5), r.triangular(10, 0))
print("triangular degenerate", repr(r.triangular(1, 1, 1)), repr(r.triangular(2.0, 2.0, 2.0)))
print("betavariate", r.betavariate(2, 3), r.betavariate(0.5, 0.5), r.betavariate(10.0, 1.0))
print("expovariate", r.expovariate(1.5), r.expovariate(), r.expovariate(-2.0))
print("gammavariate", r.gammavariate(0.5, 1), r.gammavariate(1.0, 2.0), r.gammavariate(2.5, 1.5),
      r.gammavariate(200, 1), r.gammavariate(0.01, 3.0))
print("gauss", [r.gauss() for _ in range(5)], r.gauss(10, 2), r.gauss(mu=1, sigma=0.5))
print("lognormvariate", r.lognormvariate(0, 1), r.lognormvariate(1.5, 0.25))
print("normalvariate", r.normalvariate(), r.normalvariate(5, 2), r.normalvariate(sigma=3))
print("vonmisesvariate", r.vonmisesvariate(0, 0), r.vonmisesvariate(1.0, 4.0), r.vonmisesvariate(2.0, 1e-7),
      r.vonmisesvariate(-1.0, 100.0))
print("paretovariate", r.paretovariate(1.5), r.paretovariate(3))
print("weibullvariate", r.weibullvariate(1, 2), r.weibullvariate(2.5, 0.5))

# Fraction parameters go through the Python-object arithmetic.
r = random.Random(18)
print("fraction params", r.uniform(Fraction(1, 3), Fraction(1, 10)), r.normalvariate(Fraction(1), Fraction(2)),
      r.expovariate(Fraction(3, 2)), r.gammavariate(Fraction(5, 2), Fraction(3, 2)),
      r.gauss(Fraction(1, 2), 2), repr(r.triangular(Fraction(1), Fraction(1), Fraction(1))))
print("int-valued big params", r.uniform(10**20, 10**20 + 5), r.normalvariate(2**60, 1))

# gauss_next caches the second Box-Muller value.
r = random.Random(19)
first = r.gauss()
print("gauss_next", r.gauss_next is not None, type(r.gauss_next).__name__)
r = random.Random(19)
g1 = r.gauss()
cached = r.gauss_next
g2 = r.gauss()
print("gauss pair", g1 == first, cached == g2, r.gauss_next is None)
r.seed(19)
print("seed clears gauss_next", r.gauss_next is None, r.gauss() == first)


# --- 8. getstate / setstate / pickle -----------------------------------------
r = random.Random(20)
r.gauss()
state = r.getstate()
print("state shape", state[0], len(state[1]), state[1][-1], type(state[2]).__name__)
a = [r.gauss() for _ in range(3)] + [r.random(), r.getrandbits(70)]
r.setstate(state)
b = [r.gauss() for _ in range(3)] + [r.random(), r.getrandbits(70)]
print("state round trip", a == b)

v2 = (2, tuple(x - 2**32 if x >= 2**31 else x for x in state[1][:-1]) + (state[1][-1],), state[2])
r.setstate(v2)
print("version 2 state", [r.gauss() for _ in range(3)] + [r.random(), r.getrandbits(70)] == a)

r2, r3 = random.Random(), random.Random(0)
r2.setstate(state)
r3.setstate(state)
print("state transfer", r2.random() == r3.random(), r2.gauss_next == r3.gauss_next == state[2])

r = random.Random(21)
r.random()
p = pickle.loads(pickle.dumps(r))
print("pickle", type(p).__name__, [p.random() for _ in range(3)] == [r.random() for _ in range(3)])
c = copy.copy(r)
d = copy.deepcopy(r)
print("copy", c.random() == d.random() == r.random())
red = r.__reduce__()
print("reduce", red[0] is random.Random, red[1], red[2][0])
print("dunder state", r.__getstate__() == r.getstate())
r.__setstate__(state)
print("dunder setstate", r.getstate() == state)

base = _random.Random(5)
print("_random.Random", base.random() == random.Random(5).random(), len(base.getstate()))
base.setstate(random.Random(5).getstate()[1])
print("_random setstate", base.random() == random.Random(5).random())
print("_random seed", base.seed(5) is None, base.random() == random.Random(5).random())
base.seed()
print("_random getrandbits", base.getrandbits(0), 0 <= base.getrandbits(10) < 1024)


# --- 9. SystemRandom ---------------------------------------------------------
sr = random.SystemRandom()
print("sys random", all(0.0 <= sr.random() < 1.0 for _ in range(100)))
print("sys getrandbits", sr.getrandbits(0), all(0 <= sr.getrandbits(64) < 2**64 for _ in range(20)),
      all(sr.getrandbits(1000) < 2**1000 for _ in range(5)))
print("sys randbytes", len(sr.randbytes(16)), sr.randbytes(0))
print("sys randrange", all(sr.randrange(10) in range(10) for _ in range(50)), sr.randrange(1))
print("sys choice", sr.choice("aaa"), len(sr.sample(range(100), 10)), sorted(sr.sample(range(5), 5)))
print("sys dists", isinstance(sr.gauss(), float), 0 <= sr.binomialvariate(10, 0.5) <= 10,
      isinstance(sr.uniform(1, 2), float), len(sr.choices("ab", k=4)))
x = list(range(10))
sr.shuffle(x)
print("sys shuffle", sorted(x) == list(range(10)))
print("sys seed", sr.seed(), sr.seed(1, 2, a=3))
print("sys getstate", err(sr.getstate))
print("sys setstate", err(lambda: sr.setstate(None)))
print("sys errors", err(lambda: sr.getrandbits(-1)), err(lambda: sr.getrandbits(2.5)),
      err(lambda: sr.randbytes(-1)), err(lambda: sr.randbytes(1.5)))


# --- 10. subclasses: dispatch through self -----------------------------------
class LcgRandom(random.Random):
    """random() only: integers come from _randbelow_without_getrandbits."""

    def seed(self, a=None):
        self.lcg = a or 0

    def random(self):
        self.lcg = (self.lcg * 1103515245 + 12345) % 2**31
        return self.lcg / 2**31

    def getstate(self):
        return self.lcg

    def setstate(self, state):
        self.lcg = state


print("lcg randbelow", LcgRandom._randbelow is LcgRandom._randbelow_without_getrandbits)
r = LcgRandom(7)
print("lcg randrange", [r.randrange(100) for _ in range(10)], r.randint(1, 6))
print("lcg choice", r.choice("abcdefgh"), r.sample(range(1000), 5), r.sample(range(10), 4))
x = list(range(10))
r.shuffle(x)
print("lcg shuffle", x)
print("lcg dists", r.uniform(0, 10), r.gauss(), r.normalvariate(), r.expovariate(2),
      r.gammavariate(2.5, 1), r.betavariate(2, 2), r.binomialvariate(10, 0.3),
      r.binomialvariate(100, 0.4), r.triangular(0, 1, 0.2), r.vonmisesvariate(1, 2))
print("lcg choices", r.choices("abc", k=5), r.choices("abc", [1, 2, 3], k=5))
print("lcg state", r.getstate(), copy.copy(r).lcg == r.lcg)


class BitsRandom(random.Random):
    """getrandbits() only: random() stays the MT19937 core."""

    calls = 0

    def getrandbits(self, k):
        BitsRandom.calls += 1
        return super().getrandbits(k) ^ ((1 << k) - 1 if k else 0)


print("bits randbelow", BitsRandom._randbelow is BitsRandom._randbelow_with_getrandbits)
r = BitsRandom(3)
print("bits randrange", [r.randrange(1000) for _ in range(10)], r.choice(range(50)), r.randbytes(5).hex())
print("bits random", r.random() == BitsRandom(3).random(), BitsRandom.calls > 0)


class Plain(random.Random):
    pass


print("plain", [Plain(5).random(), Plain(5).randrange(1000)] == [random.Random(5).random(), random.Random(5).randrange(1000)])
print("plain randbelow", "_randbelow" in vars(Plain))


class KwRandom(random.Random, flavor="mint"):
    pass


print("init_subclass kwargs", KwRandom(1).random() == random.Random(1).random())


class OwnBelow(random.Random):
    def _randbelow(self, n):
        return n - 1

    def random(self):
        return 0.5


print("own randbelow", [OwnBelow().randrange(10), OwnBelow().choice("xyz")], OwnBelow._randbelow(None, 4))
x = [1, 2, 3, 4]
OwnBelow().shuffle(x)
print("own shuffle", x, OwnBelow().sample(range(5), 3))


class InstanceOverride(random.Random):
    pass


r = InstanceOverride(4)
r.random = lambda: 0.25
print("instance override", r.uniform(0, 4), r.choices("abcd", k=3), r.random())

# randint dispatches to self.randrange; betavariate to self.gammavariate;
# lognormvariate to self.normalvariate; binomialvariate to itself.
class Hooks(random.Random):
    def randrange(self, start, stop=None, step=1):
        return ("randrange", start, stop)

    def gammavariate(self, alpha, beta):
        return float(alpha)

    def normalvariate(self, mu=0.0, sigma=1.0):
        return 0.0

    def binomialvariate(self, n=1, p=0.5):
        if p > 0.5:
            return super().binomialvariate(n, p)
        return 3


h = Hooks(1)
print("hooks", h.randint(1, 5), h.betavariate(2, 6), h.lognormvariate(0, 1), h.binomialvariate(10, 0.9))


# --- 11. statistics integration ----------------------------------------------
import statistics

nd = statistics.NormalDist(10, 2)
print("NormalDist.samples", nd.samples(3, seed=42))
print("NormalDist.samples str", nd.samples(3, seed="forty-two"))


# --- 12. error cases ---------------------------------------------------------
r = random.Random(1)
print(err(lambda: r.random(1)))
print(err(lambda: r.getrandbits()))
print(err(lambda: r.getrandbits(k=3)))
print(err(lambda: r.getrandbits(-1)))
print(err(lambda: r.getrandbits(1.5)))
print(err(lambda: r.getrandbits(2**40)))
print(err(lambda: _random.Random.seed(r, 1, 2)))
print(err(lambda: _random.Random.seed(r, n=1)))
print(err(lambda: _random.Random.getstate(r, 1)))
print(err(lambda: _random.Random.setstate(r)))
print(err(lambda: _random.Random.setstate(r, [1])))
print(err(lambda: _random.Random.setstate(r, (1,))))
print(err(lambda: _random.Random.setstate(r, (0,) * 624 + (625,))))
print(err(lambda: _random.Random.setstate(r, (0,) * 624 + (-1,))))
print(err(lambda: _random.Random.setstate(r, (0,) * 624 + (1.5,))))
print(err(lambda: _random.Random.setstate(r, (-1,) * 624 + (1,))))
print(err(lambda: _random.Random.setstate(r, (1.5,) * 624 + (1,))))
print(err(lambda: _random.Random(1, 2)))
print(err(lambda: _random.Random(x=1)))
print(err(lambda: random.Random(1, 2)))
print(err(lambda: random.Random.random(object())))
print(err(lambda: random.Random.choice(object(), [1])))
print(err(lambda: r.seed([1])))
print(err(lambda: r.seed({})))
print(err(lambda: r.seed(1j)))
print(err(lambda: r.seed(bytearray(b"abc"), version=1)))
print(err(lambda: r.seed(1, 2, 3)))
print(err(lambda: r.setstate((4, (), None))))
print(err(lambda: r.setstate((3, (), None))))
print(err(lambda: r.setstate((3, ()))))
print(err(lambda: r.setstate([3, (0,) * 625, None, 1])))
print(err(lambda: r.setstate(3)))
print(err(lambda: r.setstate("3")))
print(err(lambda: r.getstate(1)))
print(err(lambda: r.choice([])))
print(err(lambda: r.choice({})))
print(err(lambda: r.randrange(0)))
print(err(lambda: r.randrange(-5)))
print(err(lambda: r.randrange(5, 5)))
print(err(lambda: r.randrange(5, 1, 1)))
print(err(lambda: r.randrange(2, 10, -1)))
print(err(lambda: r.randrange(2, 10, 0)))
print(err(lambda: r.randrange(1.5)))
print(err(lambda: r.randrange(1.0)))
print(err(lambda: r.randrange(1, 2.0)))
print(err(lambda: r.randrange(1, 5, 1.0)))
print(err(lambda: r.randrange(10, step=True)))
print(err(lambda: r.randrange(10, None, 2)))
print(err(lambda: r.randrange("1")))
print(err(lambda: r.randint(1, 2.5)))
print(err(lambda: r.randint(1.5, 2)))
print(err(lambda: r.randint(1)))
print(err(lambda: r.sample(range(3), 5)))
print(err(lambda: r.sample(range(3), -1)))
print(err(lambda: r.sample({1, 2}, 1)))
print(err(lambda: r.sample({1: 2}, 1)))
print(err(lambda: r.sample([1, 2], 2.5)))
print(err(lambda: r.sample([1, 2], 1, counts=[1])))
print(err(lambda: r.sample([1, 2], 1, counts=[1.5, 1])))
print(err(lambda: r.sample([1, 2], 1, counts=[0, 0])))
print(err(lambda: r.sample([1, 2], 1, counts=[-1, 0])))
print(err(lambda: r.sample([], 1, counts=[])))
print(err(lambda: r.sample([1, 2])))
print(err(lambda: r.choices([1, 2], weights=[1], k=1)))
print(err(lambda: r.choices([1, 2], weights=[0, 0], k=1)))
print(err(lambda: r.choices([1, 2], weights=[1], cum_weights=[1], k=1)))
print(err(lambda: r.choices([1, 2], 3)))
print(err(lambda: r.choices([1, 2], 2.5)))
print(err(lambda: r.choices([1, 2], k=2.5)))
print(err(lambda: r.choices([], k=2)))
print(err(lambda: r.choices([], [], k=2)))
print(err(lambda: r.choices([1], [float("inf")])))
print(err(lambda: r.choices([1], [float("nan")])))
print(err(lambda: r.choices([1], ["a"])))
print(err(lambda: r.choices([1, 2], [-1, -2])))
print(err(lambda: r.choices([1], cum_weights=(1,))))
print(err(lambda: r.choices([1, 2], cum_weights=(), k=1)))
print(err(lambda: r.shuffle((1, 2))))
print(err(lambda: r.binomialvariate(-1)))
print(err(lambda: r.binomialvariate(5, 1.5)))
print(err(lambda: r.binomialvariate(5, -0.5)))
print(err(lambda: r.binomialvariate(5, float("nan"))))
print(err(lambda: r.gammavariate(0, 1)))
print(err(lambda: r.gammavariate(1, 0)))
print(err(lambda: r.gammavariate(-1, -1)))
print(err(lambda: r.betavariate(0, 1)))
print(err(lambda: r.expovariate(0)))
print(err(lambda: r.paretovariate(0)))
print(err(lambda: r.weibullvariate(1, 0)))
print(err(lambda: r.uniform("a", "b")))
print(err(lambda: r.normalvariate("a")))
print(err(lambda: r.lognormvariate(1000, 0)))
print(err(lambda: r.vonmisesvariate(0)))
print(err(lambda: r.randbytes(-1)))
print(err(lambda: r.randbytes(1.5)))
print(err(lambda: r.randbytes(2**29)))
# A float power that overflows raises OverflowError (ERANGE), as in CPython.
print(err(lambda: random.Random(5).paretovariate(1e-5)))
print(err(lambda: random.Random(5).weibullvariate(2.0, 1e-3)))
print(err(lambda: 2.0 ** 5000.0))
x = 10.0
print(err(lambda: x ** 400))
print(err(lambda: random.SystemRandom.seed()))
print(err(lambda: random.Random.choice()))

print("random native ok")
