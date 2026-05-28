# Parity fixture for the `errno` module.
#
# Checks that the most-used POSIX error constants are present with the
# correct integer values, and that `errorcode` maps codes back to their
# canonical names (matching CPython 3.12 on Linux).

import errno

# Basic constants
print(errno.EPERM)      # 1
print(errno.ENOENT)     # 2
print(errno.EAGAIN)     # 11
print(errno.ENOMEM)     # 12
print(errno.EACCES)     # 13
print(errno.EINVAL)     # 22
print(errno.ENOSYS)     # 38

# Alias: EWOULDBLOCK == EAGAIN on Linux
print(errno.EWOULDBLOCK == errno.EAGAIN)  # True

# Alias: EDEADLOCK == EDEADLK on Linux
print(errno.EDEADLOCK == errno.EDEADLK)  # True

# errorcode reverse mapping (unambiguous codes)
print(errno.errorcode[2])    # ENOENT
print(errno.errorcode[13])   # EACCES
print(errno.errorcode[1])    # EPERM
print(errno.errorcode[22])   # EINVAL

# errorcode canonical winner for aliased codes:
#   11: EAGAIN wins over EWOULDBLOCK
#   35: EDEADLOCK wins over EDEADLK
#   95: ENOTSUP wins over EOPNOTSUPP
print(errno.errorcode[11])   # EAGAIN
print(errno.errorcode[35])   # EDEADLOCK
print(errno.errorcode[95])   # ENOTSUP
