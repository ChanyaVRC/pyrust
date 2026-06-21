# TypeVar rejects a bivariant declaration (issue #2707 review follow-up).
from typing import TypeVar

try:
    TypeVar("T", covariant=True, contravariant=True)
except ValueError as e:
    print(type(e).__name__)
    print(e)
