# Issue #1981: an exception whose `__dict__` was replaced wholesale
# (`exc.__dict__ = d`) is dict-backed.  The traceback-chaining machinery must
# still read the carried `__traceback__` through the live dict, so a bare
# re-raise prepends the genuinely-outer frames instead of truncating the chain.


class MyErr(Exception):
    pass


def a():
    e = MyErr("boom")
    e.__dict__ = {"tag": 1}  # dict-backed from the start
    raise e


def b():
    try:
        a()
    except MyErr:
        raise  # bare re-raise must keep the a()->b() frames


def c():
    b()


# The custom dict key is attribute-accessible after the re-raise round-trip.
try:
    c()
except MyErr as caught:
    print(type(caught).__name__, caught.tag)  # MyErr 1
