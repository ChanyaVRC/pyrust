class NotACM:
    pass

try:
    with NotACM() as x:
        pass
except TypeError as e:
    print(type(e).__name__)
    print(str(e))

class HasEnterOnly:
    def __enter__(self): return self

try:
    with HasEnterOnly() as x:
        pass
except TypeError as e:
    print(type(e).__name__)
    print(str(e))

class HasExitOnly:
    def __exit__(self, *args): return False

try:
    with HasExitOnly() as x:
        pass
except TypeError as e:
    print(type(e).__name__)
    print(str(e))

# Valid context manager should still work
class GoodCM:
    def __enter__(self): return 42
    def __exit__(self, *args): return False

with GoodCM() as x:
    print(x)
