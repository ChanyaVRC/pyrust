# PEP 380 / CPython 3.12: StopIteration.value must be set to the generator's
# return value (or None when the generator falls off the end).

# --- Generator with explicit return value ---

def gen_return_int():
    yield 1
    return 42

g = gen_return_int()
next(g)
try:
    next(g)
except StopIteration as e:
    print(e.value)   # 42

# --- Generator with no return (implicit return None) ---

def gen_implicit_none():
    yield 1

g = gen_implicit_none()
next(g)
try:
    next(g)
except StopIteration as e:
    print(repr(e.value))   # None

# --- Generator returning a string ---

def gen_return_str():
    yield 1
    return "hello"

g = gen_return_str()
next(g)
try:
    next(g)
except StopIteration as e:
    print(e.value)   # hello

# --- Direct construction: StopIteration(42).value ---
print(StopIteration(42).value)   # 42

# --- Direct construction: StopIteration().value ---
print(StopIteration().value)     # None

# --- Non-generator iterator: StopIteration raised with no args → value is None ---

class EarlyStop:
    def __iter__(self):
        return self
    def __next__(self):
        raise StopIteration()

it = EarlyStop()
try:
    next(it)
except StopIteration as e:
    print(repr(e.value))   # None

# --- Generator that returns on the very first next() (no yield before return) ---

def gen_return_first():
    return 7
    yield  # make it a generator

g = gen_return_first()
try:
    next(g)
except StopIteration as e:
    print(e.value)   # 7

# --- generator.send() must preserve StopIteration.value (PEP 380) ---

def gen_send():
    yield 1
    return "send_done"

g = gen_send()
g.send(None)   # advance past first yield (same as next(g))
try:
    g.send(None)
except StopIteration as e:
    print(e.value)   # send_done

# --- generator.send() on already-exhausted generator → .value is None ---

def gen_exhaust():
    yield 1

g = gen_exhaust()
next(g)
try:
    next(g)
except StopIteration:
    pass

try:
    g.send(None)
except StopIteration as e:
    print(repr(e.value))   # None
