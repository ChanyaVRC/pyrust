try:
    def f(): return f()
    f()
except RecursionError as e:
    print(isinstance(e, RuntimeError))   # True
    print(isinstance(e, Exception))      # True
print(isinstance(NotImplementedError(), RuntimeError))  # True
print(isinstance(RuntimeError(), Exception))            # True
