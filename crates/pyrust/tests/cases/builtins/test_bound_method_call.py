# Parity test for issue #276: bound-method call-site allocation reduction.
# Exercises the common bound-method dispatch paths (int, float, str, list,
# dict, set, bytes) in a loop to verify correctness after the pos-buf and
# kw fast-path changes.

# --- int methods ---
n = 255
results = []
for _ in range(3):
    results.append(n.bit_length())
print(results)          # [8, 8, 8]
print((0).bit_length()) # 0
print((-1).bit_length()) # 1

# --- float methods ---
f = 3.14
results = []
for _ in range(3):
    results.append(f.is_integer())
print(results)           # [False, False, False]
print((1.0).is_integer()) # True
print((1.5).is_integer()) # False

# --- str methods ---
s = "hello world"
method = s.upper
results = []
for _ in range(3):
    results.append(method())
print(results)           # ['HELLO WORLD', 'HELLO WORLD', 'HELLO WORLD']
print("abc".split("b"))  # ['a', 'c']
print("  hi  ".strip())  # hi
print("{x}".format(x=42)) # 42  (kwargs path)

# --- list methods ---
lst = [3, 1, 4, 1, 5]
m_count = lst.count
results = []
for _ in range(3):
    results.append(m_count(1))
print(results)           # [2, 2, 2]

lst2 = []
m_append = lst2.append
for i in range(5):
    m_append(i)
print(lst2)              # [0, 1, 2, 3, 4]

# --- dict methods ---
d = {"a": 1, "b": 2}
m_get = d.get
results = []
for k in ["a", "b", "c"]:
    results.append(m_get(k, 0))
print(results)           # [1, 2, 0]

# --- set methods ---
seen = set()
m_add = seen.add
for x in [1, 2, 2, 3]:
    m_add(x)
print(sorted(seen))      # [1, 2, 3]
print(3 in seen)         # True

# --- bytes methods ---
b = b"hello"
m_upper = b.upper
results = []
for _ in range(3):
    results.append(m_upper())
print(results)           # [b'HELLO', b'HELLO', b'HELLO']
print(b"abc".count(b"a")) # 1

# --- int keyword-arg error path ---
try:
    (1).bit_length(base=2)
except TypeError as e:
    print("TypeError:", e)

# --- index method (tests the resolve_seq_index_pos path) ---
lst3 = [10, 20, 30]
print(lst3.index(20))    # 1
tup = (10, 20, 30)
print(tup.index(30))     # 2
