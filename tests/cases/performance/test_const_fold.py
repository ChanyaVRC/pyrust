# expressions with constant subexpressions — exercises constant folding
# Each iteration uses values that would normally require LoadConst+BinOp
# but can be folded to a single constant at compile time.
result = 0
for _ in range(1000000):
    result += 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9 + 10
assert result == 55000000
print("const-fold-sum", result)
