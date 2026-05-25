lst = [1, 2, 3]

# read: "list index out of range"
try:
    _ = lst[999]
except IndexError as e:
    print(str(e))

# del: "list assignment index out of range"
try:
    del lst[999]
except IndexError as e:
    print(str(e))

# write: "list assignment index out of range"
try:
    lst[999] = 42
except IndexError as e:
    print(str(e))

# negative OOB read
try:
    _ = lst[-999]
except IndexError as e:
    print(str(e))

# negative OOB del
try:
    del lst[-999]
except IndexError as e:
    print(str(e))

# negative OOB write
try:
    lst[-999] = 42
except IndexError as e:
    print(str(e))
