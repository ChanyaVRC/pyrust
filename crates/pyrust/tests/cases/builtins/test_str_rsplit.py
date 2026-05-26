# str.rsplit() parity: result must be in left-to-right order, matching CPython 3.12.
# When maxsplit is not given (or -1), rsplit returns the same list as split.
# When maxsplit is given, rsplit splits from the right and returns left-to-right order.

# --- explicit separator, no maxsplit (the bug case) ---
print("a,b,c,d".rsplit(","))        # ['a', 'b', 'c', 'd']
print("a,b,c,d".rsplit(",", -1))    # ['a', 'b', 'c', 'd']

# --- explicit separator, with maxsplit ---
print("a,b,c,d".rsplit(",", 1))     # ['a,b,c', 'd']
print("a,b,c,d".rsplit(",", 2))     # ['a,b', 'c', 'd']
print("a,b,c,d".rsplit(",", 100))   # ['a', 'b', 'c', 'd']

# --- whitespace separator, no maxsplit ---
print("a b c".rsplit())             # ['a', 'b', 'c']
print("a b c".rsplit(None))         # ['a', 'b', 'c']
print("a  b  c".rsplit())           # ['a', 'b', 'c']
print("  a  b  ".rsplit())          # ['a', 'b']

# --- whitespace separator, with maxsplit ---
print("a b c".rsplit(' ', 1))       # ['a b', 'c']
print("a  b  c".rsplit(None, 1))    # ['a  b', 'c']
print("  a  b  ".rsplit(None, 1))   # ['  a', 'b']

# --- edge cases ---
print("".rsplit(","))               # ['']
print("a".rsplit(","))              # ['a']
print(",".rsplit(","))              # ['', '']
print(",,".rsplit(","))             # ['', '', '']
print("a,,b".rsplit(","))           # ['a', '', 'b']
print("aXbXc".rsplit("X", 1))      # ['aXb', 'c']

# --- same result as split when maxsplit is omitted ---
s = "hello world foo bar"
print(s.split() == s.rsplit())      # True
print(s.split(" ") == s.rsplit(" "))  # True
