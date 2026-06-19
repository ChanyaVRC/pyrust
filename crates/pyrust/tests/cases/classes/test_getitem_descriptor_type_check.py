class ML(list): pass
class MT(tuple): pass
class MB(bytes): pass

# Wrong-type: each descriptor rejects other-type backing; right-type still works.
for fn, good_inst, bad_inst in [
    (list.__getitem__, ML([10, 20]), MT((1, 2))),
    (tuple.__getitem__, MT((30, 40)), ML([3, 4])),
    (bytes.__getitem__, MB(b'ab'), ML([5, 6])),
]:
    print(fn(good_inst, 0))  # should print the first element
    try:
        fn(bad_inst, 0)
        print('WRONG')
    except TypeError as e:
        print(e)
