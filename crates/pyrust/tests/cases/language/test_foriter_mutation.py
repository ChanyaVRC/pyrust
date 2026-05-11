lst = [1, 2, 3]
try:
    for x in lst:
        lst.append(x * 10)
except RuntimeError as e:
    print("RuntimeError:", e)   # RuntimeError: list changed size during iteration

lst2 = [1, 2, 3]
try:
    for x in lst2:
        lst2.pop()
except RuntimeError as e:
    print("RuntimeError:", e)   # RuntimeError: list changed size during iteration

lst3 = [1, 2, 3]
total = 0
for x in lst3:
    total += x
print(total)   # 6  (no mutation = no error)
