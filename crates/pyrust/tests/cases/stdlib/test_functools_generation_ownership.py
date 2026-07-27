import functools
import sys

old_module = functools
old_lru_cache = old_module.lru_cache


def old_update_wrapper(wrapper, wrapped):
    wrapper.factory_generation = "old"
    return wrapper


old_module.update_wrapper = old_update_wrapper


def identity(value):
    return value


old_dispatch = old_module.singledispatch(identity)
old_wrapper = old_lru_cache(identity)
old_info_type = type(old_wrapper.cache_info())
old_factory = old_lru_cache(maxsize=4)

del sys.modules["functools"]
import functools as new_module


def new_update_wrapper(wrapper, wrapped):
    wrapper.factory_generation = "new"
    return wrapper


new_module.update_wrapper = new_update_wrapper
new_dispatch = new_module.singledispatch(identity)
new_wrapper = new_module.lru_cache(identity)
new_info_type = type(new_wrapper.cache_info())
old_callable_wrapper = old_lru_cache(identity)

print(old_module is new_module)
print(old_dispatch.factory_generation, new_dispatch.factory_generation)
print(old_info_type is new_info_type)
print(type(old_wrapper) is old_module._lru_cache_wrapper)
print(type(new_wrapper) is new_module._lru_cache_wrapper)
print(type(old_callable_wrapper) is old_module._lru_cache_wrapper)

# A retained decorator still reads deliberate mutations from its own module
# generation, but never drifts to a replacement in sys.modules by itself.
old_module._lru_cache_wrapper = new_module._lru_cache_wrapper
old_module._CacheInfo = new_module._CacheInfo
patched_old_wrapper = old_factory(identity)
print(type(patched_old_wrapper) is new_module._lru_cache_wrapper)
print(type(patched_old_wrapper.cache_info()) is new_module._CacheInfo)
