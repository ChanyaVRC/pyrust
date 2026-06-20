POS = 10 ** 100
NEG = -(10 ** 100)

# --- str.count: BigInt start ---
print("abcabc".count("a", NEG))   # 2  (negative -> before start)
print("abcabc".count("a", POS))   # 0  (positive -> past end)
print("abcabc".count("a", 0, POS))  # 2  (end clamped to len)
print("abcabc".count("a", 0, NEG))  # 0  (end clamped to 0)
print("hello".count("l", 0, POS))   # 2

# --- str.find / rfind ---
print("abcabc".find("a", NEG))    # 0
print("abcabc".find("a", POS))    # -1
print("abcabc".find("c", 0, POS)) # 2
print("abcabc".rfind("a", NEG))   # 3
print("abcabc".rfind("a", POS))   # -1
print("abcabc".rfind("c", 0, POS))  # 5

# --- str.index / rindex ---
print("abcabc".index("a", NEG))   # 0
print("abcabc".rindex("a", NEG))  # 3
try:
    "abcabc".index("a", POS)
    print("WRONG")
except ValueError:
    print("ok index POS")
try:
    "abcabc".rindex("a", POS)
    print("WRONG")
except ValueError:
    print("ok rindex POS")

# --- non-ASCII str path ---
print("a£a£".count("a", NEG))     # 2
print("a£a£".find("£", POS))      # -1
print("a£a£".find("£", NEG))      # 1

# --- bytes ---
print(b"hello".count(108, NEG))   # 2
print(b"hello".count(108, POS))   # 0
print(b"hello".count(108, 0, POS))  # 2
print(b"hello".find(104, NEG))    # 0
print(b"hello".find(104, POS))    # -1
print(b"hello".rfind(108, POS))   # -1
print(b"hello".rfind(108, NEG))   # 3
print(b"hello".index(104, NEG))   # 0
print(b"hello".rindex(108, NEG))  # 3
try:
    b"hello".index(104, POS)
    print("WRONG")
except ValueError:
    print("ok bytes index POS")

# --- bytearray ---
print(bytearray(b"hello").count(108, POS))   # 0
print(bytearray(b"hello").count(108, NEG))   # 2
print(bytearray(b"hello").find(104, POS))    # -1
print(bytearray(b"hello").find(104, NEG))    # 0
print(bytearray(b"hello").rfind(108, POS))   # -1
print(bytearray(b"hello").index(104, NEG))   # 0
print(bytearray(b"hello").rindex(108, NEG))  # 3
try:
    bytearray(b"hello").rindex(108, POS)
    print("WRONG")
except ValueError:
    print("ok bytearray rindex POS")
