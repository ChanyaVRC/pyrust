# Slicing fast paths (#1964 BINARY_SLICE-equivalent rvalue read + #2066
# step==1 contiguous memcpy).  Covers list/tuple/str/bytes/range across all
# slice forms, non-ASCII char boundaries, negative step, slice-object subscript,
# and slice assignment (which must keep working through the lvalue path).

l = list(range(10))
t = tuple(range(10))
s = "abcdefghij"
b = b"abcdefghij"
r = range(0, 20, 2)

for seq in (l, t, s, b, r):
    print(seq[2:7])
    print(seq[:4])
    print(seq[5:])
    print(seq[:])
    print(seq[2:8:2])
    print(seq[::-1])
    print(seq[::2])
    print(seq[-3:])
    print(seq[:-3])
    print(seq[-7:-2])
    print(seq[100:200])      # out of range -> empty
    print(seq[5:2])          # start > stop -> empty
    print(seq[-100:100])     # clamped to full range
    print(seq[8:2:-1])       # reverse strided
    print(seq[::-2])

# Non-ASCII str: char-boundary correctness (slice indices are char-based).
u = "αβγδεζη"  # αβγδεζη
print(u[1:4])
print(u[:3])
print(u[2:])
print(u[::-1])
print(u[::2])
print(u[-2:])
print(u[1:5:2])
print(u[3:3])

# Mixed-width multibyte (varying UTF-8 lengths).
m = "aé中\U0001f600z"  # a, é, 中, 😀, z
print(m[1:4])
print(m[:2])
print(m[2:])
print(m[::-1])

# Slice-object subscript path: must still build/accept a real slice.
x = slice(1, 5)
print(l[x])
print(s[x])
y = slice(None, None, -1)
print(t[y])
print(b[y])

# Custom __getitem__ must receive a real slice object (not unpacked bounds).
class C:
    def __getitem__(self, k):
        return k
print(C()[1:5])
print(C()[1:5:2])
print(C()[::-1])
print(C()[:])

# Slice assignment (lvalue path) must keep working.
la = list(range(10))
la[2:5] = [99, 98]
print(la)
la[::2] = [0, 0, 0, 0, 0]
print(la)
del la[1:3]
print(la)

# Empty sequences.
print([][:])
print("" [0:5])
print(b""[0:5])
print(()[::-1])

# Nested list slice copies the contiguous run (shallow).
nested = [[1, 2], [3, 4], [5, 6]]
sub = nested[0:2]
sub[0].append(99)
print(nested[0])
print(sub)
