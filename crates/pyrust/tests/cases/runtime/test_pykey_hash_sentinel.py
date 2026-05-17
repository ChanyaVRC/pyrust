# Parity fixture: value_to_pykey sentinel remap and BigInt reduction (issue #503).
#
# CPython's slot_tp_hash applies:
# - `-1 → -2` sentinel remap for all __hash__ return values (fitting in i64)
# - Mersenne-prime reduction (mod 2^61-1) only when the __hash__ return
#   value overflows ssize_t (i.e. is a BigInt that doesn't fit in i64)
#
# The internal PyKey::Object hash stored in dicts/sets must match the value
# returned by hash(obj) so that any direct-hash probe finds the entry.

# --- Sentinel remap: __hash__ returning -1 must give hash -2 ---
class C:
    def __hash__(self): return -1

c = C()
print(hash(c))          # -2

d = {c: "val"}
for k in d:
    print(hash(k) == hash(c))   # True

# --- BigInt __hash__ return: CPython uses Mersenne-prime reduction ---
class D:
    def __hash__(self): return 2**100

obj = D()
print(hash(obj))        # 549755813888 (2**100 mod 2**61-1)

e = {obj: "x"}
for k in e:
    print(hash(k) == hash(obj))     # True

# --- Large i64 __hash__ return: NO Mersenne reduction (fits in ssize_t) ---
class E:
    def __hash__(self): return 2**62

obj2 = E()
print(hash(obj2))       # 4611686018427387904

f = {obj2: "y"}
for k in f:
    print(hash(k) == hash(obj2))    # True

# --- -2 is NOT remapped again (only -1 is the sentinel) ---
class F:
    def __hash__(self): return -2

obj3 = F()
print(hash(obj3))       # -2

# --- 0 stays 0 ---
class G:
    def __hash__(self): return 0

obj4 = G()
print(hash(obj4))       # 0

# --- Bool: True==1, False==0; no remap needed ---
class H:
    def __hash__(self): return True

obj5 = H()
print(hash(obj5))       # 1

# --- hash() builtin on bare int still applies Mersenne reduction ---
print(hash(-1))         # -2
print(hash(2**62))      # 2  (Mersenne reduction: 2^62 mod 2^61-1 = 2)
