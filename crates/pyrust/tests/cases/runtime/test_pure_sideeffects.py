log = []

class Logged:
    def __str__(self):
        log.append('str')
        return 'logged'
    def __repr__(self):
        log.append('repr')
        return 'Logged()'
    def __bool__(self):
        log.append('bool')
        return True
    def __iter__(self):
        log.append('iter')
        return iter([1, 2, 3])

obj = Logged()

str(obj)
print('str' in log or 'repr' in log)  # True

log.clear()
bool(obj)
print('bool' in log)  # True

log.clear()
list(obj)
print('iter' in log)  # True

log.clear()
tuple(obj)
print('iter' in log)  # True

# sorted: exercise the key-function path, which dispatches user code.
# (Direct __lt__ dispatch in sorted is tracked separately.)
log.clear()

class Item:
    def __init__(self, v):
        self.v = v

def key_fn(x):
    log.append('key')
    return x.v

sorted([Item(2), Item(1)], key=key_fn)
print('key' in log)  # True
