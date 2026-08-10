# Issue #3031: deleting a nonlocal clears the enclosing function's cell.


def outer():
    value = "bound"

    def delete_value():
        nonlocal value
        del value

    def read_value():
        return value

    try:
        delete_value()
        print("nonlocal delete: ok")
    except Exception as exc:
        print("nonlocal delete:", type(exc).__name__)

    try:
        read_value()
    except Exception as exc:
        print("free read:", type(exc).__name__)

    try:
        value
    except Exception as exc:
        print("owner read:", type(exc).__name__)


outer()


finalizer_events = []


class Tracked:
    def __del__(self):
        finalizer_events.append("finalized")


def delete_with_finalizer():
    value = Tracked()

    def delete_value():
        nonlocal value
        del value

    print("finalizer before:", finalizer_events)
    delete_value()
    print("finalizer after:", finalizer_events)


delete_with_finalizer()
