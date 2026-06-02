# sys module members added for issue #2006:
# maxunicode, abiflags, getdefaultencoding, is_finalizing, intern,
# float_info, int_info, getsizeof, implementation, builtin_module_names.
#
# DETERMINISTIC members are printed directly (parity-compared against
# CPython 3.12).  ENV/IMPL-specific members (getsizeof exact size,
# implementation.name/cache_tag/hexversion) are asserted by TYPE only so
# the fixture stays stable across versions and platforms.

import os
import sys

# --- deterministic scalars ---
print("maxunicode", sys.maxunicode)
# sys.abiflags only exists on POSIX CPython builds (AttributeError on
# Windows). pyrust always exposes it, so guard the assertion under posix to
# keep this fixture byte-identical with CPython on both platforms.
if os.name == "posix":
    print("abiflags", repr(sys.abiflags))
print("getdefaultencoding", sys.getdefaultencoding())
print("is_finalizing", sys.is_finalizing())

# --- intern: contract is intern(s) == s ---
print("intern", sys.intern("spam") == "spam")

# --- float_info: IEEE-754 doubles, stable across platforms ---
fi = sys.float_info
print("fi.max", fi.max)
print("fi.min", fi.min)
print("fi.epsilon", fi.epsilon)
print("fi.dig", fi.dig)
print("fi.mant_dig", fi.mant_dig)
print("fi.max_exp", fi.max_exp)
print("fi.min_exp", fi.min_exp)
print("fi.max_10_exp", fi.max_10_exp)
print("fi.min_10_exp", fi.min_10_exp)
print("fi.radix", fi.radix)
print("fi.rounds", fi.rounds)

# --- int_info: build constants, stable on CPython 3.12 ---
ii = sys.int_info
print("ii.bits_per_digit", ii.bits_per_digit)
print("ii.sizeof_digit", ii.sizeof_digit)
print("ii.default_max_str_digits", ii.default_max_str_digits)

# --- type-only checks (NOT value-parity-tested) ---
print("getsizeof-int>0", isinstance(sys.getsizeof(0), int) and sys.getsizeof(0) > 0)
print("getsizeof-str>0", isinstance(sys.getsizeof("hello"), int) and sys.getsizeof("hello") > 0)
print("impl-name-str", isinstance(sys.implementation.name, str))
print("impl-hexversion-int", isinstance(sys.implementation.hexversion, int))
print("builtin_module_names-tuple", isinstance(sys.builtin_module_names, tuple))
