import statistics
import math

# Averages
print(statistics.mean([1, 2, 3, 4, 5]))  # 3 (exact int)
print(statistics.mean([1.5, 2.5, 3.0]))  # 2.333...
print(statistics.mean([10]))  # 10
print(statistics.mean(x for x in [2, 4, 6]))  # 4 (generator input)
print(statistics.fmean([1, 2, 3]))  # 2.0
print(statistics.fmean([1, 2, 3], [3, 2, 1]))  # weighted

# Medians
print(statistics.median([1, 2, 3]))  # 2
print(statistics.median([1, 2, 3, 4]))  # 2.5
print(statistics.median_low([1, 2, 3, 4]))  # 2
print(statistics.median_high([1, 2, 3, 4]))  # 3
print(statistics.median(x for x in [3, 1, 2]))  # 2

# Modes
print(statistics.mode([1, 1, 2, 3]))  # 1
print(statistics.mode("aabbbccde"))  # b
print(statistics.multimode([1, 1, 2, 2, 3]))  # [1, 2]
print(statistics.multimode("aabbbccde"))  # ['b']

# Variance / stdev (exact int when integral & divisible)
print(statistics.variance([2, 4, 4, 4, 5, 5, 7, 9]))  # 4.571...
print(statistics.pvariance([2, 4, 4, 4, 5, 5, 7, 9]))  # 4 (exact int)
print(statistics.variance([1, 2, 3, 4, 5]))  # 2.5
print(statistics.pvariance([1, 2, 3, 4, 5]))  # 2 (exact int)
print(round(statistics.stdev([2, 4, 4, 4, 5, 5, 7, 9]), 5))  # 2.13809
print(round(statistics.pstdev([2, 4, 4, 4, 5, 5, 7, 9]), 5))  # 2.0

# Geometric / harmonic means
print(statistics.geometric_mean([1, 2, 4]))  # 2.0
print(statistics.harmonic_mean([1, 2, 4]))  # 1.714...
print(statistics.harmonic_mean([5]))  # 5 (single point returns input unchanged)
print(statistics.harmonic_mean([40, 60], [5, 30]))  # 56.0 (weighted)
print(statistics.median_grouped([1]))  # 1.0 (single point is a float)

# Error-message wording parity
try:
    statistics.geometric_mean([1, -2, 3])
except statistics.StatisticsError as e:
    print(str(e))  # "geometric mean ..." (space, not underscore)
try:
    statistics.harmonic_mean([1, 2, 3], [-1, -1, -1])
except statistics.StatisticsError as e:
    print(str(e))  # negative-weight uses the negative-values message
try:
    statistics.harmonic_mean([1, 2, 3], [0, 0, 0])
except statistics.StatisticsError as e:
    print(str(e))  # "Weighted sum must be positive"

# NormalDist
nd = statistics.NormalDist(2, 3)
print(nd)  # NormalDist(mu=2.0, sigma=3.0)
print(nd.mean, nd.stdev, nd.variance)
print(round(nd.cdf(2), 4))  # 0.5
print(round(nd.pdf(2), 6))

# Errors
try:
    statistics.mean([])
except statistics.StatisticsError:
    print("empty mean error")

try:
    statistics.median([])
except statistics.StatisticsError:
    print("empty median error")

try:
    statistics.variance([1])
except statistics.StatisticsError:
    print("variance needs two points")

try:
    statistics.geometric_mean([1, -2, 3])
except statistics.StatisticsError:
    print("geometric needs positives")

from statistics import StatisticsError, mean

print(issubclass(StatisticsError, ValueError))

print("statistics ok")
