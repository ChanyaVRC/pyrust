# Test that __exit__ receives (exc_type, exc_val, None) when an exception occurs
# inside a with block. pyrust has no traceback objects, so the third argument
# must be None, not the exception value itself.

class CM:
    def __enter__(self):
        return self
    def __exit__(self, exc_type, exc_val, exc_tb):
        print("type:", exc_type.__name__ if exc_type else None)
        print("val:", exc_val)
        print("tb is not exc_val:", exc_tb is not exc_val)
        return True  # suppress exception

with CM():
    raise ValueError("test error")

# No-exception path: all three args must be None
class CM2:
    def __enter__(self):
        return self
    def __exit__(self, exc_type, exc_val, exc_tb):
        print(exc_type, exc_val, exc_tb)

with CM2():
    pass
