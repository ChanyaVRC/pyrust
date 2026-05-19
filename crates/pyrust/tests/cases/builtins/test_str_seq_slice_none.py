# Parity fixture for str/list/tuple slice-arg None handling (issue #796).
# CPython 3.12 treats None for start/end as omitted (0 / len).

s = 'hello world'

# str.find / str.rfind
print(s.find('l', None, None))    # 2
print(s.find('l', None))          # 2
print(s.find('l', 3, None))       # 3
print(s.rfind('l', None, None))   # 9
print(s.rfind('l', 4, None))      # 9

# str.index / str.rindex
print(s.index('l', None, None))   # 2
print(s.rindex('l', 4, None))     # 9

# str.count
print(s.count('l', None, None))   # 3
print(s.count('l', None, 4))      # 2

# str.startswith / str.endswith
print(s.startswith('hel', None, None))  # True
print(s.startswith('wor', 6, None))     # True
print(s.endswith('rld', None, None))    # True
print(s.endswith('hel', None, 3))       # True

# Wrong type still raises TypeError with the full message
try:
    s.find('l', [])
except TypeError as e:
    print(e)

# list.index / tuple.index with None start/end
lst = [1, 2, 3, 2, 1]
print(lst.index(2, None, None))   # 1
print(lst.index(2, 2, None))      # 3
print(lst.index(1, None, 2))      # 0

t = (1, 2, 3, 2, 1)
print(t.index(2, None, None))     # 1
print(t.index(2, 2, None))        # 3
