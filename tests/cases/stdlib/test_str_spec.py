# str method CPython spec compliance
# Ref: https://docs.python.org/3/library/stdtypes.html

# --- isdigit: Unicode digits (not just ASCII) ---

# ASCII digits
print("isdigit-ascii", '5'.isdigit())           # True
print("isdigit-ascii-all", '019'.isdigit())      # True
print("isdigit-ascii-letter", 'a'.isdigit())     # False
print("isdigit-empty", ''.isdigit())             # False

# Unicode digits (superscript, Arabic-Indic, etc.)
print("isdigit-superscript", '²'.isdigit()) # True  ('²')
print("isdigit-arabic", '٢'.isdigit())      # True  ('٢')
print("isdigit-devanagari", '०'.isdigit())  # True  ('०')
print("isdigit-mixed", '5²'.isdigit())      # True  (all chars are digits)
print("isdigit-mixed-bad", '5a'.isdigit())       # False

# --- isalpha: basic cases ---

print("isalpha-ascii", 'hello'.isalpha())        # True
print("isalpha-mixed", 'hello1'.isalpha())       # False
print("isalpha-empty", ''.isalpha())             # False
print("isalpha-unicode", 'élève'.isalpha())  # True  ('élève')

# --- isalpha: combining marks must return False ---
# Unicode category Mn (non-spacing mark) — not L* — Python returns False.
# Rust char::is_alphabetic() returns True for these, causing a deviation.
print("isalpha-combining-grave", '̀'.isalpha())       # False (COMBINING GRAVE ACCENT)
print("isalpha-combining-acute", '́'.isalpha())       # False (COMBINING ACUTE ACCENT)
print("isalpha-combining-tilde", '̃'.isalpha())       # False (COMBINING TILDE)
# letter + combining mark: the combining mark alone is not alpha, but joined it is
print("isalpha-letter-plus-combining", 'é'.isalpha())  # True ('é' decomposed)

# --- startswith / endswith with tuple of prefixes ---

print("startswith-tuple-hit", 'hello'.startswith(('he', 'ho')))      # True
print("startswith-tuple-miss", 'hello'.startswith(('wo', 'ho')))     # False
print("startswith-single-tuple", 'hello'.startswith(('he',)))        # True
print("startswith-empty-tuple", 'hello'.startswith(()))              # False
print("endswith-tuple-hit", 'hello'.endswith(('lo', 'la')))          # True
print("endswith-tuple-miss", 'hello'.endswith(('la', 'le')))         # False
print("endswith-single-tuple", 'hello'.endswith(('lo',)))            # True

# --- split: empty separator must raise ValueError ---

try:
    'hello'.split('')
    print("split-empty-sep", "no-error")
except ValueError:
    print("split-empty-sep", "ValueError")

try:
    'hello'.rsplit('')
    print("rsplit-empty-sep", "no-error")
except ValueError:
    print("rsplit-empty-sep", "ValueError")

# --- count: empty substring with start/end ---

print("count-empty-full", 'abc'.count(''))        # 4  (len + 1 positions)
print("count-empty-start", 'abc'.count('', 1))    # 3  (positions in 'bc' + end)
print("count-empty-startend", 'abc'.count('', 1, 2))  # 2  (positions in 'b')
print("count-empty-startend2", 'abc'.count('', 0, 1)) # 2  (positions in 'a')

# --- join: accepts any iterable, not just list/tuple ---

# dict (iterates over keys in insertion order)
d = {'a': 1, 'b': 2, 'c': 3}
print("join-dict", '-'.join(d))                  # a-b-c

# tuple (basic check)
print("join-tuple", ' '.join(('x', 'y', 'z')))  # x y z

# --- string slice: inverted indices must return empty string ---
# Guards the missing byte_start <= byte_end check in string_slice.

print("str-slice-inverted", 'hello'[5:2])        # ''
print("str-slice-inverted2", 'hello'[3:1])       # ''
print("str-slice-equal", 'hello'[2:2])           # ''

# --- string slice: Layout B -> B chain (split result re-sliced) ---
# split() returns Layout B (zero-copy) slices. Slicing those again must
# correctly chain the offset computation without pointer underflow.

s = 'hello world'
parts = s.split(' ')
sub = parts[0][1:]        # Layout B of Layout B: 'ello'
sub2 = sub[1:]            # one more level:        'llo'
print("str-split-chain-sub", sub)          # ello
print("str-split-chain-sub2", sub2)        # llo
print("str-split-chain-orig", parts[0])    # hello (original must not be corrupted)
print("str-split-chain-src", s)            # hello world
