try:
    [][0]
except IndexError as e:
    print(e)   # list index out of range

try:
    ()[0]
except IndexError as e:
    print(e)   # tuple index out of range

try:
    ""[0]
except IndexError as e:
    print(e)   # string index out of range
