# Regression test for issue #689:
# list.index(x, start, stop) and tuple.index(x, start, stop) must raise
# ValueError (not panic) when start > stop after index normalisation.

# list — explicit inverted window: start=2, stop=1
try:
    [1, 2, 3].index(1, 2, 1)
except ValueError:
    print("list inverted window: ValueError")
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# tuple — inverted window: start=2, stop=0
try:
    (1, 2, 3).index(1, 2, 0)
except ValueError:
    print("tuple inverted window: ValueError")
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# list — negative stop that normalises below start: start=3, stop=normalise(-3,4)=1
try:
    [1, 2, 3, 4].index(1, 3, -3)
except ValueError:
    print("list negative stop below start: ValueError")
except Exception as e:
    print("wrong class:", type(e).__name__, e)

# Normal case — start <= stop must still work
print([1, 2, 3].index(1, 0, 2))   # 0
print([1, 2, 3].index(2, 1, 3))   # 1
print((1, 2, 3).index(3, 0, 3))   # 2
