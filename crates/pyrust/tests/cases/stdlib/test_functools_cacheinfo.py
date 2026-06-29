import functools


@functools.lru_cache(maxsize=128)
def square(x):
    return x * x


_ = square(5)
_ = square(5)  # second call is a hit
info = square.cache_info()

print(type(info).__module__)  # functools
print(type(info).__name__)  # CacheInfo
print(info.hits)  # 1
print(info.misses)  # 1
print(info.maxsize)  # 128
print(info.currsize)  # 1

# Named tuple behavior
print(isinstance(info, tuple))  # True
print(len(info))  # 4

print("CacheInfo module ok")
