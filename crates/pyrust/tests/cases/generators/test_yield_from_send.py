# PEP 380 yield-from delegation: send(), throw(), and return-value capture.

# --- 1. Basic value forwarding ---

def inner_basic():
    yield 1
    yield 2
    yield 3

def outer_basic():
    yield from inner_basic()

print(list(outer_basic()))


# --- 2. send() forwarding into sub-generator ---

def inner_send():
    x = yield 'first'
    y = yield f'got {x}'
    yield f'got {y}'

def outer_send():
    yield from inner_send()

g = outer_send()
print(next(g))
print(g.send('hello'))
print(g.send('world'))


# --- 3. StopIteration.value (return value of sub-generator) ---

def sub_return():
    yield 10
    return 'sub_result'

def outer_return():
    result = yield from sub_return()
    yield f'result={result}'

g = outer_return()
print(next(g))
print(next(g))


# --- 4. throw() forwarded into sub-generator ---

def inner_throw():
    try:
        yield 'before'
    except ValueError as e:
        yield f'caught: {e}'

def outer_throw():
    yield from inner_throw()

g = outer_throw()
print(next(g))
print(g.throw(ValueError('oops')))


# --- 5. throw() propagates when inner does not catch it ---

def inner_noncatch():
    yield 'x'

def outer_noncatch():
    yield from inner_noncatch()

g = outer_noncatch()
next(g)
try:
    g.throw(RuntimeError('boom'))
except RuntimeError as e:
    print(f'propagated: {e}')


# --- 6. close() forwarded to sub-generator ---

def inner_close():
    try:
        yield 'inner'
    except GeneratorExit:
        print('inner got GeneratorExit')
        raise

def outer_close():
    yield from inner_close()

g = outer_close()
next(g)
g.close()
print('after close')


# --- 7. Nested yield from (chain of sub-generators) ---

def a():
    yield 1
    return 'a_done'

def b():
    r = yield from a()
    yield f'b got {r}'
    return 'b_done'

def c():
    r = yield from b()
    yield f'c got {r}'

print(list(c()))


# --- 8. yield from a non-generator iterable (list): result is None ---

def outer_list():
    result = yield from [10, 20, 30]
    yield f'list result={result}'

print(list(outer_list()))


# --- 9. Bare return (value is None) ---

def sub_none():
    yield 1
    return

def outer_none():
    result = yield from sub_none()
    yield f'none result={result}'

print(list(outer_none()))
