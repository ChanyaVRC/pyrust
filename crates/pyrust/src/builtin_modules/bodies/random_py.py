"""Pure-Python random module for pyrust (issue #2785).

CPython's ``random`` module is a thin Python layer over the ``_random.Random``
C extension, which implements the MT19937 Mersenne Twister.  pyrust has no C
extension layer, so this module embeds a pure-Python MT19937 generator and
builds the full public API (``random``, ``randint``, ``choice``, ``shuffle``,
``sample``, ``gauss``, …) on top of it.

The algorithm is the reference MT19937, so a given integer seed produces a
deterministic, reproducible sequence.  The numeric values do NOT match
CPython's ``random`` for the same seed: CPython seeds MT19937 through
``init_by_array`` over the key derived from the seed, and pyrust uses the
classic scalar ``init_genrand`` seeding.  Parity is therefore on the API
contract (types, ranges, reproducibility, uniqueness), not on exact draws.

Reference: <https://docs.python.org/3/library/random.html>
"""

from math import (
    log as _log,
    sqrt as _sqrt,
    cos as _cos,
    sin as _sin,
    acos as _acos,
    pi as _pi,
    e as _e,
)
from os import urandom as _urandom
from operator import index as _index

__all__ = [
    "Random",
    "seed",
    "random",
    "uniform",
    "randint",
    "randrange",
    "choice",
    "choices",
    "shuffle",
    "sample",
    "getstate",
    "setstate",
    "getrandbits",
    "gauss",
    "normalvariate",
    "expovariate",
    "triangular",
    "betavariate",
    "gammavariate",
    "lognormvariate",
    "paretovariate",
    "weibullvariate",
    "vonmisesvariate",
]

_N = 624
_M = 397
_MATRIX_A = 0x9908B0DF
_UPPER_MASK = 0x80000000
_LOWER_MASK = 0x7FFFFFFF
_MASK32 = 0xFFFFFFFF

_TWOPI = 2.0 * _pi
_LOG4 = _log(4.0)
_SG_MAGICCONST = 1.0 + _log(4.5)
_BPF = 53  # bits in a float mantissa
_RECIP_BPF = 2.0 ** (-_BPF)
_NV_MAGICCONST = 4 * _e ** (-0.5) / _sqrt(2.0)


class Random:
    """Mersenne-Twister-based random number generator.

    Mirrors the public surface of CPython's ``random.Random`` (the methods this
    module documents); the generator core is the reference MT19937.
    """

    VERSION = 3  # used by getstate/setstate

    def __init__(self, x=None):
        self.gauss_next = None
        self.seed(x)

    # -- core MT19937 ----------------------------------------------------

    def _init_genrand(self, s):
        mt = [0] * _N
        mt[0] = s & _MASK32
        for i in range(1, _N):
            prev = mt[i - 1]
            mt[i] = (1812433253 * (prev ^ (prev >> 30)) + i) & _MASK32
        self._mt = mt
        self._mti = _N

    def _genrand_uint32(self):
        mt = self._mt
        if self._mti >= _N:
            for kk in range(_N - _M):
                y = (mt[kk] & _UPPER_MASK) | (mt[kk + 1] & _LOWER_MASK)
                mt[kk] = mt[kk + _M] ^ (y >> 1) ^ (_MATRIX_A if (y & 1) else 0)
            for kk in range(_N - _M, _N - 1):
                y = (mt[kk] & _UPPER_MASK) | (mt[kk + 1] & _LOWER_MASK)
                mt[kk] = mt[kk + (_M - _N)] ^ (y >> 1) ^ (_MATRIX_A if (y & 1) else 0)
            y = (mt[_N - 1] & _UPPER_MASK) | (mt[0] & _LOWER_MASK)
            mt[_N - 1] = mt[_M - 1] ^ (y >> 1) ^ (_MATRIX_A if (y & 1) else 0)
            self._mti = 0

        y = mt[self._mti]
        self._mti += 1
        y ^= y >> 11
        y ^= (y << 7) & 0x9D2C5680
        y ^= (y << 15) & 0xEFC60000
        y ^= y >> 18
        return y & _MASK32

    # -- seeding ---------------------------------------------------------

    def seed(self, a=None, version=2):
        """Initialize the generator from a hashable object.

        ``None`` seeds from OS entropy.  An ``int`` seeds deterministically.
        ``str``/``bytes``/``bytearray`` are folded into an int seed.
        """
        if a is None:
            a = int.from_bytes(_urandom(8), "big")
        elif isinstance(a, (str, bytes, bytearray)):
            # CPython folds str/bytes through SHA-512; pyrust has no hashlib in
            # the import graph here, so fold the raw bytes to an int instead.
            # The exact value differs from CPython (the whole MT seeding does),
            # but it is deterministic per input, which is the contract.
            if isinstance(a, str):
                a = a.encode()
            a = int.from_bytes(a, "big") if a else 0
        elif isinstance(a, float):
            # CPython accepts floats; fold the value to a deterministic int.
            a = hash(a)
        elif not isinstance(a, int):
            raise TypeError(
                "The only supported seed types are: None,\n"
                "int, float, str, bytes, and bytearray."
            )
        self._init_genrand(abs(int(a)))
        self.gauss_next = None

    def getstate(self):
        """Return an opaque object capturing the generator's internal state."""
        return (self.VERSION, tuple(self._mt) + (self._mti,), self.gauss_next)

    def setstate(self, state):
        """Restore the internal state from a ``getstate()`` result."""
        version = state[0]
        if version != self.VERSION:
            raise ValueError(
                "state with version %s passed to Random.setstate() of version %s"
                % (version, self.VERSION)
            )
        internalstate = state[1]
        if len(internalstate) != _N + 1:
            raise ValueError("state vector is the wrong size")
        self._mt = list(internalstate[:_N])
        self._mti = internalstate[_N]
        self.gauss_next = state[2]

    # -- bit / float primitives -----------------------------------------

    def getrandbits(self, k):
        """Return a non-negative int with ``k`` random bits."""
        # CPython routes k through the integer protocol, so non-int args raise
        # the standard operator.index TypeError and __index__ objects work.
        k = _index(k)
        if k < 0:
            raise ValueError("number of bits must be non-negative")
        if k == 0:
            return 0
        words = (k + 31) // 32
        result = 0
        shift = 0
        bits_left = k
        for _ in range(words):
            r = self._genrand_uint32()
            if bits_left < 32:
                r >>= 32 - bits_left
            result |= r << shift
            shift += 32
            bits_left -= 32
        return result

    def random(self):
        """Return a float in [0.0, 1.0)."""
        a = self._genrand_uint32() >> 5
        b = self._genrand_uint32() >> 6
        return (a * 67108864.0 + b) * _RECIP_BPF

    # -- integer methods -------------------------------------------------

    def _randbelow(self, n):
        """Return a random int in [0, n) for n > 0 using rejection sampling."""
        k = n.bit_length()
        r = self.getrandbits(k)
        while r >= n:
            r = self.getrandbits(k)
        return r

    def randrange(self, start, stop=None, step=1):
        """Choose a random item from ``range(start, stop[, step])``."""
        # Mirrors CPython 3.12's Random.randrange: arguments go through
        # operator.index (so floats raise TypeError, not ValueError) and the
        # empty-range messages echo the original start/stop/step.
        istart = _index(start)
        if stop is None:
            if step != 1:
                raise TypeError("Missing a non-None stop argument")
            if istart > 0:
                return self._randbelow(istart)
            raise ValueError("empty range for randrange()")

        istop = _index(stop)
        width = istop - istart
        istep = _index(step)
        if istep == 1:
            if width > 0:
                return istart + self._randbelow(width)
            raise ValueError("empty range in randrange(%s, %s)" % (start, stop))

        if istep > 0:
            n = (width + istep - 1) // istep
        elif istep < 0:
            n = (width + istep + 1) // istep
        else:
            raise ValueError("zero step for randrange()")
        if n <= 0:
            raise ValueError(
                "empty range in randrange(%s, %s, %s)" % (start, stop, step)
            )
        return istart + istep * self._randbelow(n)

    def randint(self, a, b):
        """Return a random integer N such that ``a <= N <= b``."""
        return self.randrange(a, b + 1)

    # -- sequence methods -----------------------------------------------

    def choice(self, seq):
        """Choose a random element from a non-empty sequence."""
        if len(seq) == 0:
            raise IndexError("Cannot choose from an empty sequence")
        return seq[self._randbelow(len(seq))]

    def shuffle(self, x):
        """Shuffle list ``x`` in place."""
        for i in range(len(x) - 1, 0, -1):
            j = self._randbelow(i + 1)
            x[i], x[j] = x[j], x[i]

    def sample(self, population, k, counts=None):
        """Return a ``k`` length list of unique elements from ``population``."""
        if not isinstance(population, (list, tuple, str, bytes, range)):
            raise TypeError(
                "Population must be a sequence.  For dicts or sets, use sorted(d)."
            )
        n = len(population)
        if counts is not None:
            cum = list(_accumulate(counts))
            if len(cum) != n:
                raise ValueError(
                    "The number of counts does not match the population"
                )
            total = cum[-1]
            if not isinstance(total, int):
                raise TypeError("Counts must be integers")
            if total <= 0:
                raise ValueError("Total of counts must be greater than zero")
            selections = self.sample(range(total), k=k)
            return [population[_bisect(cum, s)] for s in selections]
        if not 0 <= k <= n:
            raise ValueError("Sample larger than population or is negative")
        result = [None] * k
        setsize = 21
        if k > 5:
            setsize += 4 ** _ceil(_log(k * 3, 4))
        if n <= setsize:
            pool = list(population)
            for i in range(k):
                j = self._randbelow(n - i)
                result[i] = pool[j]
                pool[j] = pool[n - i - 1]
        else:
            selected = set()
            for i in range(k):
                j = self._randbelow(n)
                while j in selected:
                    j = self._randbelow(n)
                selected.add(j)
                result[i] = population[j]
        return result

    def choices(self, population, weights=None, cum_weights=None, k=1):
        """Return a ``k`` sized list of elements chosen with replacement."""
        n = len(population)
        if cum_weights is None:
            if weights is None:
                n_float = float(n)
                return [
                    population[_floor(self.random() * n_float)] for _ in range(k)
                ]
            try:
                cum_weights = list(_accumulate(weights))
            except TypeError:
                raise TypeError("weights must be a sequence of numbers") from None
        elif weights is not None:
            raise TypeError("Cannot specify both weights and cumulative weights")
        if len(cum_weights) != n:
            raise ValueError("The number of weights does not match the population")
        total = cum_weights[-1] + 0.0
        if total <= 0.0:
            raise ValueError("Total of weights must be greater than zero")
        hi = n - 1
        return [
            population[_bisect(cum_weights, self.random() * total, 0, hi)]
            for _ in range(k)
        ]

    # -- real-valued distributions --------------------------------------

    def uniform(self, a, b):
        """Return a random float N such that ``a <= N <= b`` (or b <= N <= a)."""
        return a + (b - a) * self.random()

    def triangular(self, low=0.0, high=1.0, mode=None):
        """Triangular distribution."""
        u = self.random()
        try:
            c = 0.5 if mode is None else (mode - low) / (high - low)
        except ZeroDivisionError:
            return low
        if u > c:
            u = 1.0 - u
            c = 1.0 - c
            low, high = high, low
        return low + (high - low) * _sqrt(u * c)

    def normalvariate(self, mu=0.0, sigma=1.0):
        """Normal distribution (Kinderman and Monahan method)."""
        random = self.random
        while True:
            u1 = random()
            u2 = 1.0 - random()
            z = _NV_MAGICCONST * (u1 - 0.5) / u2
            zz = z * z / 4.0
            if zz <= -_log(u2):
                break
        return mu + z * sigma

    def gauss(self, mu=0.0, sigma=1.0):
        """Gaussian distribution (faster than normalvariate, not thread-safe)."""
        random = self.random
        z = self.gauss_next
        self.gauss_next = None
        if z is None:
            x2pi = random() * _TWOPI
            g2rad = _sqrt(-2.0 * _log(1.0 - random()))
            z = _cos(x2pi) * g2rad
            self.gauss_next = _sin(x2pi) * g2rad
        return mu + z * sigma

    def lognormvariate(self, mu, sigma):
        """Log normal distribution."""
        return _e ** self.normalvariate(mu, sigma)

    def expovariate(self, lambd=1.0):
        """Exponential distribution."""
        return -_log(1.0 - self.random()) / lambd

    def vonmisesvariate(self, mu, kappa):
        """Circular data distribution."""
        random = self.random
        if kappa <= 1e-6:
            return _TWOPI * random()
        s = 0.5 / kappa
        r = s + _sqrt(1.0 + s * s)
        while True:
            u1 = random()
            z = _cos(_pi * u1)
            d = z / (r + z)
            u2 = random()
            if u2 < 1.0 - d * d or u2 <= (1.0 - d) * _e ** d:
                break
        q = 1.0 / r
        f = (q + z) / (1.0 + q * z)
        u3 = random()
        if u3 > 0.5:
            theta = (mu + _acos(f)) % _TWOPI
        else:
            theta = (mu - _acos(f)) % _TWOPI
        return theta

    def gammavariate(self, alpha, beta):
        """Gamma distribution (not the gamma function)."""
        if alpha <= 0.0 or beta <= 0.0:
            raise ValueError("gammavariate: alpha and beta must be > 0.0")
        random = self.random
        if alpha > 1.0:
            ainv = _sqrt(2.0 * alpha - 1.0)
            bbb = alpha - _LOG4
            ccc = alpha + ainv
            while True:
                u1 = random()
                if not 1e-7 < u1 < 0.9999999:
                    continue
                u2 = 1.0 - random()
                v = _log(u1 / (1.0 - u1)) / ainv
                x = alpha * _e ** v
                z = u1 * u1 * u2
                rr = bbb + ccc * v - x
                if rr + _SG_MAGICCONST - 4.5 * z >= 0.0 or rr >= _log(z):
                    return x * beta
        elif alpha == 1.0:
            return -_log(1.0 - random()) * beta
        else:
            while True:
                u = random()
                b = (_e + alpha) / _e
                p = b * u
                if p <= 1.0:
                    x = p ** (1.0 / alpha)
                else:
                    x = -_log((b - p) / alpha)
                u1 = random()
                if p > 1.0:
                    if u1 <= x ** (alpha - 1.0):
                        break
                elif u1 <= _e ** (-x):
                    break
            return x * beta

    def betavariate(self, alpha, beta):
        """Beta distribution."""
        y = self.gammavariate(alpha, 1.0)
        if y:
            return y / (y + self.gammavariate(beta, 1.0))
        return 0.0

    def paretovariate(self, alpha):
        """Pareto distribution."""
        u = 1.0 - self.random()
        return u ** (-1.0 / alpha)

    def weibullvariate(self, alpha, beta):
        """Weibull distribution."""
        u = 1.0 - self.random()
        return alpha * (-_log(u)) ** (1.0 / beta)


# -- small helpers (module-private) -------------------------------------


def _floor(x):
    i = int(x)
    return i - 1 if (i > x) else i


def _ceil(x):
    i = int(x)
    return i + 1 if (i < x) else i


def _accumulate(iterable):
    total = None
    first = True
    for v in iterable:
        if first:
            total = v
            first = False
        else:
            total = total + v
        yield total


def _bisect(a, x, lo=0, hi=None):
    if hi is None:
        hi = len(a)
    while lo < hi:
        mid = (lo + hi) // 2
        if x < a[mid]:
            hi = mid
        else:
            lo = mid + 1
    return lo


# -- module-level singleton + bound functions ---------------------------

_inst = Random()

seed = _inst.seed
random = _inst.random
uniform = _inst.uniform
triangular = _inst.triangular
randint = _inst.randint
choice = _inst.choice
randrange = _inst.randrange
sample = _inst.sample
shuffle = _inst.shuffle
choices = _inst.choices
normalvariate = _inst.normalvariate
lognormvariate = _inst.lognormvariate
expovariate = _inst.expovariate
vonmisesvariate = _inst.vonmisesvariate
gammavariate = _inst.gammavariate
gauss = _inst.gauss
betavariate = _inst.betavariate
paretovariate = _inst.paretovariate
weibullvariate = _inst.weibullvariate
getstate = _inst.getstate
setstate = _inst.setstate
getrandbits = _inst.getrandbits
