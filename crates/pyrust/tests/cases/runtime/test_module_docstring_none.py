# No docstring — __doc__ should be None in globals() and via the name binding.
print(__doc__)
print(globals()['__doc__'] is None)
print('__doc__' in globals())
