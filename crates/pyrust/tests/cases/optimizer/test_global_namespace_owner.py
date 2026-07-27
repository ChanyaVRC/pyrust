import _global_namespace_owner as imported


# The import binding and this padding assignment advance the parent
# Interpreter's historical per-interpreter counter to 2. write() used to move
# it to 3, colliding with read()'s child-Interpreter cache entry.
padding = None
imported.write()
print(imported.read())

# An imported function's globals provider belongs to its captured root, not to
# the Interpreter that happens to call it.
imported.g["x"] = 4
print(imported.read())

# Function.__globals__ is another Python-visible alias to the same imported
# root provider and must disable/synchronize caches just like globals().
imported_function_globals = imported.read.__globals__
imported_function_globals["x"] = 5
print(imported.read.__globals__ is imported_function_globals)
print(imported.read())


local_x = 1


def read_local_x():
    return local_x


function_globals = read_local_x.__globals__
function_globals["local_x"] = 6
print(read_local_x())

import sys

frame_globals = sys._getframe().f_globals
frame_globals["local_x"] = 7
print(read_local_x())
