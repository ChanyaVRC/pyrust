# Parity fixture: hash() for complex numbers (issue #1398)
#
# CPython 3.12 algorithm (Objects/complexobject.c complex_hash):
#   hash_re = _Py_HashDouble(re)   (as Py_uhash_t)
#   hash_im = _Py_HashDouble(im)   (as Py_uhash_t)
#   combined = hash_re + 1000003 * hash_im  (wrapping u64)
#   result = combined as i64; if -1 return -2

# Basic values
print(hash(0+0j))   # 0
print(hash(0+1j))   # 1000003
print(hash(1+0j))   # 1
print(hash(3+4j))   # 4000015

# Zero imaginary: hash(x+0j) == hash(x) for int/float
print(hash(1+0j) == hash(1))
print(hash(2+0j) == hash(2))
print(hash(3.14+0j) == hash(3.14))
print(hash(1.5+0j) == hash(1.5))

# hash consistency across numeric types
print(hash(1+0j) == hash(1) == hash(1.0))
print(hash(2+0j) == hash(2) == hash(2.0))

# Sentinel remap: -1 -> -2
print(hash(-1+0j))     # -2 (because hash(-1.0) = -2 via sentinel remap)
print(hash(-1+1j))     # 1000001

# Complex as dict keys
d = {1+2j: "a", 3+4j: "b"}
print(d[1+2j])
print(d[3+4j])
print(len(d))

# Complex with zero imag in dict: cross-type lookup
d2 = {1+0j: "one"}
print(d2[1])     # cross-type: 1+0j == 1 and hash(1+0j) == hash(1)
print(d2[1.0])   # cross-type: 1+0j == 1.0 and hash(1+0j) == hash(1.0)

# Int key lookups complex value
d3 = {1: "int"}
print(d3[1+0j])

# Complex in sets
s = {1+2j, 3+4j, 1+0j}
print(len(s))
print(1 in {1+0j})     # True: 1 == 1+0j and hash parity
print(1.0 in {1+0j})   # True: 1.0 == 1+0j and hash parity

# frozenset containing complex
fs = frozenset({1+2j, 3+4j})
print(len(fs))
print(1+2j in fs)
print(3+4j in fs)

# Complex in tuple (hashing a tuple that contains complex)
t = (1+2j, 3+4j)
print(hash(t) == hash((1+2j, 3+4j)))
print(hash(t) == hash((complex(1, 2), complex(3, 4))))
