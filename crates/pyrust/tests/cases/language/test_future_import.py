# Test that `from __future__ import X` is accepted as a no-op for all valid
# feature names, and that invalid names raise SyntaxError.
from __future__ import annotations
from __future__ import division
from __future__ import print_function
from __future__ import unicode_literals
from __future__ import generators
from __future__ import nested_scopes
from __future__ import absolute_import
from __future__ import with_statement
from __future__ import barry_as_FLUFL
from __future__ import generator_stop

# Multiple names in a single statement.
from __future__ import annotations, division, print_function

x = 1
print(x)
print("ok")
