# random module — parity (issue #2785).
#
# pyrust ships a pure-Python MT19937 with CPython's exact int-seed algorithm, so
# for int/float/None seeds the numeric stream is bit-identical to CPython.  This
# fixture asserts the exact draws for fixed int seeds (the critical guarantee),
# plus the version-stable contract: types, ranges, reproducibility, uniqueness,
# and the error wording.
import random

# --- bit-exact MT19937 stream (must byte-match CPython 3.12) ----------------
random.seed(42)
print(random.random())
random.seed(42)
print(random.getrandbits(32))
random.seed(42)
print(random.getrandbits(64))
random.seed(42)
print(random.randint(1, 100))
random.seed(42)
print(random.sample(range(100), 5))
random.seed(42)
_sh = list(range(10))
random.shuffle(_sh)
print(_sh)
random.seed(0)
print(random.random())
random.seed(1099511627783)
print([random.random() for _ in range(3)])
random.seed(0.5)
print(random.random())
print(random.Random(99).random())
print([random.Random(7).randint(0, 1000000) for _ in range(3)])

# --- reproducibility: same seed -> same sequence ---------------------------
random.seed(42)
seq1 = [random.random() for _ in range(5)]
random.seed(42)
seq2 = [random.random() for _ in range(5)]
print(seq1 == seq2)

# --- random() range --------------------------------------------------------
random.seed(1)
print(all(0.0 <= random.random() < 1.0 for _ in range(100)))

# --- randint / randrange ---------------------------------------------------
print(random.randint(5, 5))
print(all(1 <= random.randint(1, 10) <= 10 for _ in range(100)))
print(all(random.randrange(10) in range(10) for _ in range(100)))
print(all(random.randrange(2, 12, 2) % 2 == 0 for _ in range(100)))
print(all(random.randrange(2, 12, 2) in range(2, 12, 2) for _ in range(100)))

# --- choice / shuffle / sample --------------------------------------------
lst = [1, 2, 3, 4, 5]
print(all(random.choice(lst) in lst for _ in range(50)))

lst2 = list(range(10))
random.shuffle(lst2)
print(sorted(lst2) == list(range(10)))

s = random.sample(range(100), 10)
print(len(s) == 10 and len(set(s)) == 10)
print(all(x in range(100) for x in s))
print(random.sample(range(5), 0) == [])

# sample with counts
sc = random.sample(["a", "b"], k=3, counts=[2, 2])
print(len(sc) == 3 and all(c in ("a", "b") for c in sc))

# --- choices ---------------------------------------------------------------
c = random.choices([1, 2, 3], weights=[1, 2, 3], k=100)
print(len(c) == 100 and all(x in (1, 2, 3) for x in c))
c2 = random.choices([1, 2, 3], cum_weights=[1, 3, 6], k=50)
print(len(c2) == 50 and all(x in (1, 2, 3) for x in c2))
c3 = random.choices("abc", k=10)
print(len(c3) == 10 and all(x in "abc" for x in c3))

# --- uniform / triangular --------------------------------------------------
print(all(1.0 <= random.uniform(1.0, 3.0) <= 3.0 for _ in range(100)))
print(all(0.0 <= random.triangular() <= 1.0 for _ in range(100)))
print(all(2.0 <= random.triangular(2.0, 8.0, 5.0) <= 8.0 for _ in range(100)))

# --- real-valued distributions return floats -------------------------------
print(isinstance(random.gauss(0.0, 1.0), float))
print(isinstance(random.normalvariate(0.0, 1.0), float))
print(random.expovariate(1.0) >= 0.0)
print(random.betavariate(2.0, 3.0) >= 0.0)
print(random.gammavariate(2.0, 1.0) >= 0.0)
print(random.paretovariate(1.0) >= 1.0)
print(random.weibullvariate(1.0, 1.0) >= 0.0)
print(isinstance(random.lognormvariate(0.0, 1.0), float))
print(isinstance(random.vonmisesvariate(0.0, 1.0), float))

# --- getrandbits -----------------------------------------------------------
print(random.getrandbits(0) == 0)
print(all(0 <= random.getrandbits(8) < 256 for _ in range(100)))
print(0 <= random.getrandbits(100) < (1 << 100))
# getrandbits coerces via the integer protocol (bool / __index__ accepted)
print(random.getrandbits(True) in (0, 1))

# --- getstate / setstate ---------------------------------------------------
state = random.getstate()
draw = random.random()
random.setstate(state)
print(random.random() == draw)

# --- independent Random instances ------------------------------------------
r1 = random.Random(99)
r2 = random.Random(99)
print([r1.random() for _ in range(3)] == [r2.random() for _ in range(3)])
print(random.Random(7).randint(0, 1000000) == random.Random(7).randint(0, 1000000))

# --- error wording (must match CPython 3.12 byte-for-byte) -----------------
def err(fn):
    try:
        fn()
        return "no-error"
    except Exception as e:
        return "%s: %s" % (type(e).__name__, e)

print(err(lambda: random.choice([])))
print(err(lambda: random.randrange(0)))
print(err(lambda: random.randrange(5, 5)))
print(err(lambda: random.randrange(2, 10, -1)))
print(err(lambda: random.randrange(2, 10, 0)))
print(err(lambda: random.randrange(1.5)))
print(err(lambda: random.sample(range(3), 5)))
print(err(lambda: random.sample(range(3), -1)))
print(err(lambda: random.getrandbits(-1)))
print(err(lambda: random.getrandbits(1.5)))
print(err(lambda: random.choices([1, 2], weights=[1], k=1)))
print(err(lambda: random.choices([1, 2], weights=[0, 0], k=1)))
print(err(lambda: random.choices([1, 2], weights=[1], cum_weights=[1], k=1)))

print("random ok")
