# ImportError / ModuleNotFoundError .name and .path attributes (issue #946).
# CPython 3.12: ImportError.__init__ stores the module name as .name and the
# module file path as .path.  For a non-existent top-level module, .path is
# None in both CPython and pyrust.

# --- bare import: ModuleNotFoundError ---
try:
    import xyz_nonexistent_abc
except ModuleNotFoundError as e:
    print(e.name)
    print(e.path)

# --- bare import caught as ImportError (subclass) ---
try:
    import xyz_nonexistent_abc
except ImportError as e:
    print(e.name)
    print(e.path)

# --- args[0] is the message string ---
try:
    import xyz_nonexistent_abc
except ModuleNotFoundError as e:
    print(e.args[0])

# --- isinstance checks still hold ---
try:
    import xyz_nonexistent_abc
except ImportError as e:
    print(isinstance(e, ImportError))
    print(isinstance(e, ModuleNotFoundError))
    print(type(e).__name__)

# --- .name type and value ---
try:
    import xyz_nonexistent_abc
except ModuleNotFoundError as e:
    print(type(e.name).__name__)
    print(e.name == 'xyz_nonexistent_abc')
    print(e.path is None)

# --- from-module import: .name is the module name ---
try:
    from xyz_nonexistent_abc import something
except ModuleNotFoundError as e:
    print(e.name)
    print(e.path is None)
