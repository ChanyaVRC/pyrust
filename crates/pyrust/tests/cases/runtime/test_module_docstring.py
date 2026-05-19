"""Module doc here."""

# Module-level docstring must be stored in __doc__ and visible through
# both the name binding and globals().
print(__doc__)
print(globals()['__doc__'])
print('__doc__' in globals())

# Without a docstring the value is None (tested by importing this module
# concept — see test_module_docstring_none.py).
