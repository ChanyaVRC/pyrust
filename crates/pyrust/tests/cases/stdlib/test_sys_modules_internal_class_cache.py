"""Internal module-owned classes survive an unusable visible sys.modules."""

import sys
import typing


original_modules = sys.modules
print(repr(typing.List[int]))
try:
    sys.modules = 1
    print(repr(typing.List[str]))
    del sys.modules
    print(repr(typing.List[bytes]))
finally:
    sys.modules = original_modules
