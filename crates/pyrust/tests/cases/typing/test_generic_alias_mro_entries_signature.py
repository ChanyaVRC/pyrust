# `_GenericAlias.__mro_entries__` is an ordinary `(self, bases)` Python
# method. Its `bases` argument is unused by typing, but normal signature
# validation still applies.
from typing import Generic, TypeVar


T = TypeVar("T")
alias = Generic[T]

print(alias.__mro_entries__((alias,)) == (Generic,))
print(alias.__mro_entries__(bases=(alias,)) == (Generic,))


def report(call):
    try:
        call()
    except TypeError as exc:
        print(type(exc).__name__, str(exc))
    else:
        print("NO ERROR")


report(lambda: alias.__mro_entries__())
report(lambda: alias.__mro_entries__((alias,), object()))
report(lambda: alias.__mro_entries__((alias,), bases=(alias,)))
report(lambda: alias.__mro_entries__((alias,), unknown=True))
