BIG = 1 << 70

for seq in ([1, 2, 3], 'abc', (1, 2, 3), b'abc'):
    try:
        _ = seq[BIG]
        print('WRONG', type(seq).__name__)
    except IndexError as e:
        print('ok', str(e))  # "cannot fit 'int' into an index-sized integer"

# Negative big index, same message
for seq in ([1, 2, 3], 'abc'):
    try:
        _ = seq[-BIG]
        print('WRONG', type(seq).__name__)
    except IndexError as e:
        print('ok', str(e))

# Normal indices still work
print([1, 2, 3][2])  # 3
print('abc'[-1])     # c
