"""Basic statistics module.

A pure-Python subset of CPython's `statistics` module (targeting 3.12
behaviour), adapted to avoid dependencies pyrust does not yet ship
(`fractions`, `decimal`).  CPython uses `fractions.Fraction` internally for
exact rational arithmetic; here the averages are computed with floating
point and `mean` returns an exact `int` when every input is integral and the
sum divides evenly (matching `statistics.mean([1, 2, 3, 4, 5]) == 3`).

Reference: https://docs.python.org/3/library/statistics.html
"""

import math

__all__ = [
    "StatisticsError",
    "mean",
    "fmean",
    "geometric_mean",
    "harmonic_mean",
    "median",
    "median_low",
    "median_high",
    "median_grouped",
    "mode",
    "multimode",
    "pstdev",
    "pvariance",
    "stdev",
    "variance",
    "NormalDist",
]


class StatisticsError(ValueError):
    pass


def _coerce_data(data):
    """Materialise *data* into a list."""
    if not isinstance(data, list):
        data = list(data)
    return data


def _all_integral(data):
    for x in data:
        if not isinstance(x, int):
            return False
    return True


def mean(data):
    """Return the sample arithmetic mean of *data*.

    Returns an exact ``int`` when the inputs are all integers and the total
    divides evenly by the count; otherwise returns a ``float``.
    """
    data = _coerce_data(data)
    n = len(data)
    if n < 1:
        raise StatisticsError("mean requires at least one data point")
    if _all_integral(data):
        total = sum(data)
        if total % n == 0:
            return total // n
        return total / n
    return math.fsum(data) / n


def fmean(data, weights=None):
    """Convert *data* to floats and compute the arithmetic mean."""
    if weights is None:
        data = _coerce_data(data)
        n = len(data)
        if n < 1:
            raise StatisticsError("fmean requires at least one data point")
        return math.fsum(data) / n
    data = list(data)
    weights = list(weights)
    if len(data) != len(weights):
        raise StatisticsError("data and weights must be the same length")
    num = math.fsum(x * w for x, w in zip(data, weights))
    den = math.fsum(weights)
    if den == 0:
        raise StatisticsError("sum of weights must be non-zero")
    return num / den


def geometric_mean(data):
    """Convert *data* to floats and compute the geometric mean."""
    data = _coerce_data(data)
    n = len(data)
    if n < 1:
        raise StatisticsError(
            "geometric mean requires a non-empty dataset containing positive numbers"
        )
    total = 0.0
    for x in data:
        if x <= 0:
            raise StatisticsError(
                "geometric mean requires a non-empty dataset containing positive numbers"
            )
        total += math.log(x)
    return math.exp(total / n)


def harmonic_mean(data, weights=None):
    """Return the harmonic mean of *data*."""
    data = _coerce_data(data)
    errmsg = "harmonic mean does not support negative values"
    n = len(data)
    if n < 1:
        raise StatisticsError("harmonic_mean requires at least one data point")
    elif n == 1 and weights is None:
        x = data[0]
        if x < 0:
            raise StatisticsError(errmsg)
        return x
    if weights is None:
        weights = [1] * n
        sum_weights = n
    else:
        weights = list(weights)
        if len(weights) != n:
            raise StatisticsError("Number of weights does not match data size")
        for w in weights:
            if w < 0:
                raise StatisticsError(errmsg)
        sum_weights = math.fsum(weights)
    for x in data:
        if x < 0:
            raise StatisticsError(errmsg)
    try:
        total = math.fsum(w / x if w else 0 for w, x in zip(weights, data))
    except ZeroDivisionError:
        return 0
    if total <= 0:
        raise StatisticsError("Weighted sum must be positive")
    return sum_weights / total


def median(data):
    """Return the median (middle value) of numeric *data*."""
    data = sorted(data)
    n = len(data)
    if n == 0:
        raise StatisticsError("no median for empty data")
    if n % 2 == 1:
        return data[n // 2]
    i = n // 2
    return (data[i - 1] + data[i]) / 2


def median_low(data):
    """Return the low median of numeric *data*."""
    data = sorted(data)
    n = len(data)
    if n == 0:
        raise StatisticsError("no median for empty data")
    if n % 2 == 1:
        return data[n // 2]
    return data[n // 2 - 1]


def median_high(data):
    """Return the high median of numeric *data*."""
    data = sorted(data)
    n = len(data)
    if n == 0:
        raise StatisticsError("no median for empty data")
    return data[n // 2]


def median_grouped(data, interval=1.0):
    """Estimate the median for numeric data grouped/binned by *interval*."""
    data = sorted(data)
    n = len(data)
    if n == 0:
        raise StatisticsError("no median for empty data")
    x = data[n // 2]
    # Find the position of the leftmost x and the count of x.
    i = 0
    while i < n and data[i] < x:
        i += 1
    cf = i
    f = data.count(x)
    interval = float(interval)
    x = float(x)
    l = x - interval / 2.0
    return l + interval * (n / 2.0 - cf) / f


def multimode(data):
    """Return a list of the most frequently occurring values."""
    from collections import Counter

    counts = Counter(iter(data)).most_common()
    if not counts:
        return []
    maxcount = counts[0][1]
    return [value for value, count in counts if count == maxcount]


def mode(data):
    """Return the most common data point from discrete or nominal *data*."""
    from collections import Counter

    pairs = Counter(iter(data)).most_common(1)
    if not pairs:
        raise StatisticsError("no mode for empty data")
    return pairs[0][0]


def _ss(data, c=None):
    """Return the sum of square deviations of *data* about the mean *c*."""
    data = _coerce_data(data)
    if c is None:
        c = fmean(data)
    total = math.fsum((float(x) - c) ** 2 for x in data)
    # Apply a correction for rounding error (CPython does this too).
    total -= math.fsum(float(x) - c for x in data) ** 2 / len(data)
    return total


def _ss_integral(data):
    """Return ``n * sum(deviations**2)`` exactly for integral *data*.

    ``n * Σ(x - mean)**2`` equals ``n * Σx**2 - (Σx)**2``, an exact integer
    when every value is an ``int``.  Returning this scaled quantity lets the
    caller divide by ``n * (n - 1)`` (sample) or ``n * n`` (population) and
    recover an exact ``int`` when the variance is integral, matching the
    `Fraction`-based exact arithmetic CPython uses.
    """
    n = len(data)
    return n * sum(x * x for x in data) - sum(data) ** 2


def _exact_or_float(num, den):
    """Return ``num // den`` when it divides evenly, else ``num / den``."""
    if num % den == 0:
        return num // den
    return num / den


def variance(data, xbar=None):
    """Return the sample variance of *data*."""
    data = _coerce_data(data)
    n = len(data)
    if n < 2:
        raise StatisticsError("variance requires at least two data points")
    if xbar is None and _all_integral(data):
        return _exact_or_float(_ss_integral(data), n * (n - 1))
    return _ss(data, xbar) / (n - 1)


def pvariance(data, mu=None):
    """Return the population variance of *data*."""
    data = _coerce_data(data)
    n = len(data)
    if n < 1:
        raise StatisticsError("pvariance requires at least one data point")
    if mu is None and _all_integral(data):
        return _exact_or_float(_ss_integral(data), n * n)
    return _ss(data, mu) / n


def stdev(data, xbar=None):
    """Return the square root of the sample variance."""
    data = _coerce_data(data)
    n = len(data)
    if n < 2:
        raise StatisticsError("stdev requires at least two data points")
    return math.sqrt(_ss(data, xbar) / (n - 1))


def pstdev(data, mu=None):
    """Return the square root of the population variance."""
    data = _coerce_data(data)
    n = len(data)
    if n < 1:
        raise StatisticsError("pstdev requires at least one data point")
    return math.sqrt(_ss(data, mu) / n)


class NormalDist:
    """Normal distribution of a random variable."""

    __slots__ = ("_mu", "_sigma")

    def __init__(self, mu=0.0, sigma=1.0):
        if sigma < 0.0:
            raise StatisticsError("sigma must be non-negative")
        self._mu = float(mu)
        self._sigma = float(sigma)

    @property
    def mean(self):
        return self._mu

    @property
    def median(self):
        return self._mu

    @property
    def mode(self):
        return self._mu

    @property
    def stdev(self):
        return self._sigma

    @property
    def variance(self):
        return self._sigma**2.0

    @classmethod
    def from_samples(cls, data):
        data = _coerce_data(data)
        xbar = fmean(data)
        return cls(xbar, stdev(data, xbar))

    def pdf(self, x):
        if self._sigma == 0.0:
            raise StatisticsError("pdf() not defined when sigma is zero")
        variance = self._sigma**2.0
        return math.exp((x - self._mu) ** 2.0 / (-2.0 * variance)) / math.sqrt(
            2.0 * math.pi * variance
        )

    def cdf(self, x):
        if self._sigma == 0.0:
            raise StatisticsError("cdf() not defined when sigma is zero")
        return 0.5 * (1.0 + math.erf((x - self._mu) / (self._sigma * math.sqrt(2.0))))

    def __add__(self, other):
        if isinstance(other, NormalDist):
            return NormalDist(
                self._mu + other._mu,
                math.hypot(self._sigma, other._sigma),
            )
        return NormalDist(self._mu + other, self._sigma)

    def __sub__(self, other):
        if isinstance(other, NormalDist):
            return NormalDist(
                self._mu - other._mu,
                math.hypot(self._sigma, other._sigma),
            )
        return NormalDist(self._mu - other, self._sigma)

    def __mul__(self, other):
        return NormalDist(self._mu * other, self._sigma * abs(other))

    def __truediv__(self, other):
        return NormalDist(self._mu / other, self._sigma / abs(other))

    def __eq__(self, other):
        if not isinstance(other, NormalDist):
            return NotImplemented
        return self._mu == other._mu and self._sigma == other._sigma

    def __hash__(self):
        return hash((self._mu, self._sigma))

    def __repr__(self):
        return f"{type(self).__name__}(mu={self._mu!r}, sigma={self._sigma!r})"
