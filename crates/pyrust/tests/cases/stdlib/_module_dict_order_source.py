"""Source-backed module used by `test_module_dict_order.py`.

Its own namespace order is asserted from the importer: a filesystem module's
dict must present the same five-slot CPython head as a built-in module's.
"""

first = 1


def second():
    return first
