# Parity fixture for __future__ module and `from __future__ import X` name binding.
#
# Verifies:
#   - `from __future__ import X` binds X in the module namespace.
#   - `from __future__ import X as Y` binds Y (not X).
#   - `import __future__` works and exposes all ten feature names.
#   - All ten features have the correct CPython 3.12 repr.
#   - CO_xxx integer constants match CPython 3.12.

# All future imports must appear before any other statements.
from __future__ import annotations
from __future__ import division as div

import __future__

# The name `annotations` is bound in this scope.
print(annotations)

# Alias: `div` is bound to the division feature.
print(div)

# All ten feature names accessible as module attributes.
for name in __future__.all_feature_names:
    print(name, '=', getattr(__future__, name))

# CO_xxx constants.
print(__future__.CO_NESTED)
print(__future__.CO_GENERATOR_ALLOWED)
print(__future__.CO_FUTURE_DIVISION)
print(__future__.CO_FUTURE_ABSOLUTE_IMPORT)
print(__future__.CO_FUTURE_WITH_STATEMENT)
print(__future__.CO_FUTURE_PRINT_FUNCTION)
print(__future__.CO_FUTURE_UNICODE_LITERALS)
print(__future__.CO_FUTURE_BARRY_AS_BDFL)
print(__future__.CO_FUTURE_GENERATOR_STOP)
print(__future__.CO_FUTURE_ANNOTATIONS)

# The class name exposed by the instance.
print(type(__future__.annotations).__name__)

# Module name.
print(__future__.__name__)
