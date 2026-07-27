def read_before_assignment():
    print(value)
    yield None
    value = 1


def read_after_delete():
    value = 2
    yield value
    del value
    print(value)


for generator in (read_before_assignment(), read_after_delete()):
    try:
        while True:
            next(generator)
    except StopIteration:
        pass
    except Exception as exc:
        print(type(exc).__name__, str(exc))
