# CPython raises TypeError when exec/eval tries to look up a builtin name and
# __builtins__ is a non-subscriptable type (None, int, float, ...).  Code that
# does not access any builtin name succeeds even with an invalid __builtins__.

# exec with None builtins — no builtin lookup needed, must succeed
g = {'__builtins__': None}
exec('x = 1', g)
print('exec x=1 None builtins: ok')

# exec with int builtins — no builtin lookup needed, must succeed
g2 = {'__builtins__': 42}
exec('x = 1', g2)
print('exec x=1 int builtins: ok')

# exec with None builtins — builtin lookup triggers TypeError
try:
    exec('len([])', {'__builtins__': None})
    print('exec len None: ok')
except TypeError as e:
    print('exec len None TypeError:', e)

# exec with int builtins — builtin lookup triggers TypeError
try:
    exec('len([])', {'__builtins__': 42})
    print('exec len int: ok')
except TypeError as e:
    print('exec len int TypeError:', e)

# eval with None builtins — no builtin lookup needed
result = eval('1 + 1', {'__builtins__': None})
print('eval 1+1 None builtins:', result)

# eval with None builtins — builtin lookup triggers TypeError
try:
    eval('len([])', {'__builtins__': None})
    print('eval len None: ok')
except TypeError as e:
    print('eval len None TypeError:', e)

# exec with empty dict builtins — builtin lookup raises NameError
try:
    exec('len([])', {'__builtins__': {}})
    print('exec len empty dict: ok')
except NameError as e:
    print('exec len empty dict NameError:', e)

# exec with no globals at all — must work normally
exec('x = 1')
print('exec no globals: ok')

# exec with dict builtins — must work normally
exec('x = len([1, 2, 3])', {'__builtins__': {'len': len}})
print('exec with dict builtins: ok')
