try:
    (1).to_bytes(10**100, 'big')
except OverflowError as e:
    print(str(e))

try:
    (1).to_bytes(10**100, 'little', signed=True)
except OverflowError as e:
    print(str(e))
