class ML(list): pass
class MT(tuple): pass
class MB(bytes): pass
class MD(dict): pass

# Wrong-type: each descriptor rejects other-type backing; right-type still works.
for fn, good_inst, good_key, bad_inst in [
    (list.__getitem__, ML([10, 20]), 0, MT((1, 2))),
    (tuple.__getitem__, MT((30, 40)), 0, ML([3, 4])),
    (bytes.__getitem__, MB(b'ab'), 0, ML([5, 6])),
    (dict.__getitem__, MD({'k': 7}), 'k', ML([8, 9])),
]:
    print(fn(good_inst, good_key))  # should print the first element / value
    try:
        fn(bad_inst, 0)
        print('WRONG')
    except TypeError as e:
        print(e)
