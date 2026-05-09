# while loop with integer comparison condition — exercises CmpJump fusion
i = 0
n = 2000000
s = 0
while i < n:
    s += i
    i += 1
assert s == 1999999000000
print("while-cmp-sum", s)
