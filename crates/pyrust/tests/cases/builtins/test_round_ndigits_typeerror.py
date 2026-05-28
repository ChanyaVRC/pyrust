try:
    round(7, 1.5)
except TypeError as e:
    print(e)

try:
    round(7, "x")
except TypeError as e:
    print(e)

try:
    round(7, [])
except TypeError as e:
    print(e)

print(round(7, 2))
print(round(7.5))
print(round(7.5, 0))
