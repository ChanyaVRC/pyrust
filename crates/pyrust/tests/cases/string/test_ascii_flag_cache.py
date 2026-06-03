# Tests for the cached ASCII-ness flag on the string header (#2124).
#
# #2116 added an ASCII fast path for str index/slice/find but recomputed
# `is_ascii()` (O(n)) on every op.  #2124 caches the result on the string
# header so the check is O(1).  The flag is set eagerly at construction and
# propagated/lazily-computed for slices.  A WRONG flag would corrupt
# char-boundary handling, so every construction path is exercised here by
# indexing / slicing / find on its result.  All output must be byte-identical
# to CPython 3.12, for both ASCII and non-ASCII strings.

def probe(s):
    n = len(s)
    parts = [n]
    if n:
        parts += [s[0], s[-1], s[n // 2], s[: n // 2], s[n // 2 :], s[::2], s[::-1]]
    parts += [s.find("a"), s.find("e"), s.rfind("a"), s.count("a"), s.count("")]
    parts += [s.startswith("a"), s.endswith("a"), s.index("a") if "a" in s else -1]
    return parts


# --- construction paths: each result's flag must be correct ---
print(probe("hello world abc"))            # ascii literal
print(probe("café résumé ☃ abc"))     # non-ascii literal
print(probe("abc" + "déf"))                # concat (mixed)
print(probe("xy" + "z" * 3))               # concat + repeat (ascii)
print(probe("ab" * 6))                     # repeat (ascii)
print(probe("é" * 5))                      # repeat (non-ascii)
print(probe(",".join(["a", "bé", "c"])))   # join (mixed)
print(probe("-".join(["ab", "cd"])))       # join (ascii)
x = 7
print(probe(f"a={x} wörld"))               # f-string (non-ascii)
print(probe("{}-{}".format("ab", "çd")))   # str.format (mixed)
print(probe("%s/%d/%s" % ("ab", 9, "éf"))) # printf (mixed)
print(probe("Hello".upper()))              # upper (ascii)
print(probe("ÄÖÜ".lower()))                # lower (non-ascii)
print(probe("a.b.a".replace(".", "é")))    # replace (mixed)
print(probe("  abc  ".strip()))            # strip (ascii)
print(probe("hello world".title()))        # title (ascii)
print(probe(chr(233) + "abc"))             # chr non-ascii + ascii
print(probe(chr(97) + chr(98)))            # chr ascii
print(probe(bytes([97, 98, 99]).decode())) # bytes.decode (ascii)
print(probe("héllo".encode("utf-8").decode("utf-8")))  # round-trip (non-ascii)

# --- slices (Layout B): ASCII parent propagates, non-ASCII parent is lazy ---
big_ascii = "abcdefghij" * 10
print(probe(big_ascii[5:35]))              # slice of ascii => ascii (propagated)
big_mixed = "xé" * 20
print(probe(big_mixed[2:30]))              # slice of non-ascii => lazy (non-ascii)
print(probe(big_mixed[0:1]))               # slice of non-ascii that IS ascii ("x")
print(probe(("abcé" * 8)[0:3]))            # ascii slice of non-ascii parent
sl = big_mixed[4:40]
print(probe(sl[2:20]))                     # slice of a slice

# --- repeated indexing of a lazy (non-ascii) slice: cached flag stays correct ---
s = "αβγδεζηθικ" * 200                      # 2000 non-ascii chars
view = s[50:1950]                          # Layout B slice, lazy flag
print([view[i] for i in range(0, len(view), 311)])
print(view[0], view[-1], view[len(view) // 2], view.find("γ"), view.count("β"))

# --- empty / single-char edge cases ---
print(probe(""))
print(probe("a"))
print(probe("é"))

# --- out-of-range still raises IndexError on cached-ascii strings ---
t = "abcdef"
try:
    t[100]
except IndexError as e:
    print("IndexError:", e)
u = "αβγ"
try:
    u[100]
except IndexError as e:
    print("IndexError:", e)
