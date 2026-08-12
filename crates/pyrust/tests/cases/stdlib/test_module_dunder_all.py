import decimal
import fractions
import operator
import pprint
import string
import textwrap
import types


modules = (operator, string, types, textwrap)

for module in modules:
    print(module.__name__, module.__all__)
    print(all(hasattr(module, name) for name in module.__all__))

    namespace = {}
    exec("from " + module.__name__ + " import *", namespace)
    imported_names = [name for name in namespace if name != "__builtins__"]
    print(imported_names == module.__all__)
    print(all(namespace[name] is getattr(module, name) for name in module.__all__))


# The shared boundary also subsumes an existing ad-hoc publication and exposes
# other source-defined public lists.  Conversely, a source with no public list
# must remain absent; CPython's decimal module deliberately has no __all__.
for module in (fractions, pprint):
    print(module.__name__, module.__all__)
    print(all(hasattr(module, name) for name in module.__all__))

print("decimal", hasattr(decimal, "__all__"))
