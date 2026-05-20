# Parity fixture for Insn::Concat (single-allocation string chain concat).
# The optimizer's pass_concat_merge replaces chains of BinOp(Add) on strings
# with a single Concat instruction when the chain is 3+ operands long.

# Basic 4-string chain (the motivating case).
a, b, c, d = "hello", " ", "world", "!"
print(a + b + c + d)   # hello world!

# 3-string chain.
x, y, z = "foo", "bar", "baz"
print(x + y + z)       # foobarbaz

# 5-string chain.
parts = ["a", "b", "c", "d", "e"]
print(parts[0] + parts[1] + parts[2] + parts[3] + parts[4])  # abcde

# Mixed: one operand is str(int) (exercises the fallback path in Concat).
num = 42
s = "num=" + str(num) + "!"
print(s)               # num=42!

# Empty string in the chain.
e1, e2, e3 = "", "x", ""
print(e1 + e2 + e3)   # x

# Unicode strings.
u1, u2, u3 = "é", "té", "!"
print(u1 + u2 + u3)   # été!

# Two-string chain (not merged by pass_concat_merge — falls through to BinOp).
p, q = "ab", "cd"
print(p + q)           # abcd

# Chain where result is assigned and printed (not just discarded).
result = "one" + " " + "two" + " " + "three"
print(result)          # one two three
