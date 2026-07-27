# CPython 3.12 accepts any Python object as property.__set_name__'s `name`.
# The object is retained and rendered with repr only if an accessor is absent.
class Owner:
    pass


class DisplayName:
    def __repr__(self):
        return "<field>"


def getter_error(name):
    descriptor = property()
    descriptor.__set_name__(Owner, name)
    try:
        descriptor.__get__(Owner(), Owner)
    except AttributeError as exc:
        print(type(exc).__name__, str(exc))


getter_error(123)
getter_error(DisplayName())
