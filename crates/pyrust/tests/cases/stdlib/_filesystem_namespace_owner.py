x = 1
__all__ = ["x", "read"]


def read():
    return x


def write(value):
    global x
    x = value


def remove():
    global x
    del x
