BIG = 1 << 70
L = [1, 2, 3]

for method_call, desc in [
    (lambda: L.insert(BIG, 9), 'insert big'),
    (lambda: L.insert(-BIG, 9), 'insert neg-big'),
    (lambda: [1, 2, 3].pop(BIG), 'pop big'),
    (lambda: [1, 2, 3].pop(-BIG), 'pop neg-big'),
    (lambda: bytearray(b'abc').insert(BIG, 1), 'bytearray insert big'),
    (lambda: bytearray(b'abc').insert(-BIG, 1), 'bytearray insert neg-big'),
    (lambda: bytearray(b'abc').pop(BIG), 'bytearray pop big'),
    (lambda: bytearray(b'abc').pop(-BIG), 'bytearray pop neg-big'),
]:
    try:
        method_call()
        print('WRONG', desc)
    except OverflowError as e:
        print('ok', str(e))  # "Python int too large to convert to C ssize_t"
    except Exception as e:
        print('WRONG TYPE', type(e).__name__, desc, str(e))

# Sanity: normal usage unaffected
L2 = [1, 2, 3]
L2.insert(1, 9)
print(L2)    # [1, 9, 2, 3]
print([1, 2, 3].pop(0))  # 1
ba = bytearray(b'abc')
ba.insert(1, 9)
print(ba)    # bytearray(b'a\t bc')
print(bytearray(b'abc').pop(0))  # 97
