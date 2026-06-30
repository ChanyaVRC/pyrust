# Issue #2771: with metaclass __repr__ dispatch fixed, repr(EnumClass) /
# str(EnumClass) run EnumType.__repr__ instead of the default `<class '...'>`.
from enum import Enum, IntEnum


class Color(Enum):
    RED = 1
    GREEN = 2


class Pri(IntEnum):
    LOW = 1
    HIGH = 2


print(repr(Color))  # <enum 'Color'>
print(str(Color))   # <enum 'Color'>  (type.__str__ delegates to __repr__)
print(f"{Color!r}")  # <enum 'Color'>
print(f"{Color}")    # <enum 'Color'>

print(repr(Pri))  # <enum 'Pri'>
print(str(Pri))   # <enum 'Pri'>

# Inside a container the class repr is dispatched too.
print([Color, Pri])  # [<enum 'Color'>, <enum 'Pri'>]

# Member repr/str are unaffected by the class-repr change.
print(repr(Color.RED))  # <Color.RED: 1>
print(str(Color.RED))   # Color.RED
