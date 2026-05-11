# while True loop: sum 1..1_000_000 using constant-true fast-path
n = 1000000
acc = 0
while True:
    if n == 0:
        break
    acc += n
    n -= 1
assert acc == 500000500000
print("while-true-sum", acc)
