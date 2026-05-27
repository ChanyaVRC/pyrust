# Test: `return e` directly inside an `except … as e:` block (issue #1445).
# CPython wraps the except body in an implicit try/finally that deletes `e`
# on exit.  The return value must be loaded *before* the delete fires.

# Case 1: basic return of except variable
def f1():
    try:
        raise ValueError('x')
    except ValueError as e:
        return e

r = f1()
print(type(r).__name__, str(r))

# Case 2: return e as the only statement in except
def f2():
    try:
        raise RuntimeError('orig')
    except RuntimeError as e:
        return e

r = f2()
print(type(r).__name__, str(r))

# Case 3: except var deleted after normal (non-returning) exit
def f3():
    try:
        raise ValueError('del')
    except ValueError as e:
        pass
    try:
        _ = e
        return 'bad: e still exists'
    except NameError:
        return 'ok: e deleted'

print(f3())

# Case 4: return e with prior use in same except block
def f4():
    try:
        raise TypeError('only')
    except TypeError as e:
        s = str(e)
        return e

r = f4()
print(type(r).__name__, str(r))

# Case 5: return the inner e when nested except clauses shadow the outer one
def f5():
    try:
        raise ValueError('outer')
    except ValueError as e:
        try:
            raise KeyError('inner')
        except KeyError as e:
            return e

r = f5()
print(type(r).__name__, str(r))
