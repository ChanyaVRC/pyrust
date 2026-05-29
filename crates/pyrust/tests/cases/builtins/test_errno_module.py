# Parity fixture for the `errno` module.
#
# Tests only the POSIX-portable subset whose values are identical across
# Linux and Windows CPython 3.12 (avoids Linux-specific aliases like
# EDEADLOCK/ENOTSUP and platform-divergent values like ENOSYS).

import errno

# Basic constants (same value on all POSIX-compliant platforms)
print(errno.EPERM)      # 1
print(errno.ENOENT)     # 2
print(errno.EAGAIN)     # 11
print(errno.ENOMEM)     # 12
print(errno.EACCES)     # 13
print(errno.EINVAL)     # 22

# ENOSYS exists on all platforms (value is platform-dependent)
print(errno.ENOSYS > 0)   # True

# errorcode reverse mapping (unambiguous, platform-portable codes)
print(errno.errorcode[2])    # ENOENT
print(errno.errorcode[13])   # EACCES
print(errno.errorcode[1])    # EPERM
print(errno.errorcode[22])   # EINVAL

# errorcode canonical winner for code 11: EAGAIN wins over EWOULDBLOCK
# (both Linux and Windows agree on this)
print(errno.errorcode[11])   # EAGAIN
