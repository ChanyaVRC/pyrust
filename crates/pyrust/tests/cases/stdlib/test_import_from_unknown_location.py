# Parity fixture for issue #957.
#
# CPython 3.12 appends "(unknown location)" to ImportError messages raised by
# `from mod import missing_attr` when the module has no physical file path
# (C-extension / built-in modules).  pyrust has no physical paths for any
# module, so it should always append "(unknown location)".

try:
    from math import _nonexistent_xyz_attr
except ImportError as e:
    print(str(e))
    # cannot import name '_nonexistent_xyz_attr' from 'math' (unknown location)

try:
    from sys import _nonexistent_xyz_attr
except ImportError as e:
    print(str(e))
    # cannot import name '_nonexistent_xyz_attr' from 'sys' (unknown location)
