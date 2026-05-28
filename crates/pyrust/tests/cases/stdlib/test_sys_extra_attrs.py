# Parity fixture: sys.executable, sys.byteorder, sys.prefix, sys.exec_prefix,
# sys.base_prefix, sys.base_exec_prefix, sys.hexversion, sys.copyright,
# sys.flags (issue #1127).

import sys

# sys.executable — string; value differs between implementations, so only
# check the type.
print(isinstance(sys.executable, str))      # True

# sys.byteorder — 'little' or 'big' depending on platform endianness.
print(isinstance(sys.byteorder, str))       # True
print(sys.byteorder in ('little', 'big'))   # True

# sys.prefix / exec_prefix / base_prefix / base_exec_prefix — strings.
print(isinstance(sys.prefix, str))           # True
print(isinstance(sys.exec_prefix, str))      # True
print(isinstance(sys.base_prefix, str))      # True
print(isinstance(sys.base_exec_prefix, str)) # True

# sys.hexversion — int; check that major.minor encoded in it matches
# version_info (the exact patch/serial may differ between pyrust and CPython).
print(isinstance(sys.hexversion, int))      # True
major = (sys.hexversion >> 24) & 0xFF
minor = (sys.hexversion >> 16) & 0xFF
print(major == sys.version_info.major)      # True
print(minor == sys.version_info.minor)      # True

# sys.copyright — string.
print(isinstance(sys.copyright, str))       # True

# sys.flags — the named-tuple-like object with per-flag integer/bool fields.
print(hasattr(sys, 'flags'))               # True
print(isinstance(sys.flags.debug, int))    # True
print(isinstance(sys.flags.optimize, int)) # True
print(isinstance(sys.flags.verbose, int))  # True
print(isinstance(sys.flags.interactive, int))  # True
