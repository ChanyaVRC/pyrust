# open(closefd=...) uses ordinary Python truth-value conversion.  Omitted
# closefd defaults to True, while an explicitly supplied None is false.

events = []


class Flag:
    def __bool__(self):
        events.append("__bool__")
        return False


def show(label, value):
    try:
        handle = open(__file__, closefd=value)
    except Exception as exc:
        print(label, type(exc).__name__)
    else:
        print(label, "opened")
        handle.close()


show("custom-false", Flag())
show("explicit-none", None)
print("events", events)
