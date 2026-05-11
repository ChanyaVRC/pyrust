x = 10
if x > 5:
    print("big")
else:
    print("small")

y = 3
while y > 0:
    print(y)
    y = y - 1

def check(n):
    if n == 0:
        return "zero"
    return "nonzero"

print(check(0))
print(check(1))
