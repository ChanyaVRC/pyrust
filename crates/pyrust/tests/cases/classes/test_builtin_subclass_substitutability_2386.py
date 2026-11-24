# Issue #2386: Full builtin-subclass substitutability parity sweep across all
# 11 subclassable builtin kinds (int, float, complex, str, bytes, bytearray,
# list, tuple, dict, set, frozenset).


class I(int):
    pass


class F(float):
    pass


class C(complex):
    pass


class S(str):
    pass


class B(bytes):
    pass


class BA(bytearray):
    pass


class L(list):
    pass


class T(tuple):
    pass


class D(dict):
    pass


class St(set):
    pass


class FS(frozenset):
    pass


# --- 1. Construction, isinstance, type, extra attributes ---
i = I(42)
f = F(3.5)
c = C(3, 4)
s = S("hello")
b = B(b"hello")
ba = BA(b"hello")
l = L([1, 2, 3])
t = T((1, 2, 3))
d = D({"a": 1, "b": 2})
st = St([1, 2])
fs = FS([1, 2])

all_objs = [i, f, c, s, b, ba, l, t, d, st, fs]
all_bases = [int, float, complex, str, bytes, bytearray, list, tuple, dict, set, frozenset]
print("1_types", [type(x).__name__ for x in all_objs])
print("1_isinstance", [isinstance(x, base) for x, base in zip(all_objs, all_bases)])
for idx, x in enumerate(all_objs):
    x.tag = idx
print("1_tags", [x.tag for x in all_objs])

# --- 2. Methods (bound, captured bound, unbound) ---
print("2_int", i.bit_length(), i.bit_count(), i.real, i.imag, i.numerator, i.denominator, i.conjugate())
print("2_int_bound", I(7).bit_length(), int.bit_length(I(7)))
m_int = I(7).bit_length
print("2_int_captured", m_int())

print("2_float", f.is_integer(), F(4.0).is_integer(), f.as_integer_ratio(), f.real, f.imag, f.conjugate())
m_flt = f.is_integer
print("2_float_captured", m_flt(), float.is_integer(f), float.hex(f), float.as_integer_ratio(f))

print("2_complex", c.real, c.imag, c.conjugate(), type(c.conjugate()).__name__)
m_cplx = c.conjugate
print("2_complex_captured", m_cplx(), complex.conjugate(c))

print("2_str", s.upper(), s.replace(S("l"), S("L")), s.startswith(S("he")), s.find(S("lo")))
print("2_str_join_fmt", S("-").join([S("a"), S("b")]), S("{0}:{k}").format(I(1), k=S("x")))
m_str = s.upper
print("2_str_captured", m_str(), str.upper(s))

print("2_bytes", b.upper(), b.replace(B(b"l"), B(b"L")), b.startswith(B(b"he")), b.decode("ascii"), b.hex())
print("2_bytes_join", B(b"-").join([B(b"a"), BA(b"b")]))
m_bytes = b.upper
print("2_bytes_captured", m_bytes(), bytes.upper(b))

ba2 = BA(b"ab")
ba2.append(99)
ba2.extend(B(b"de"))
print("2_ba_mut", bytes(ba2), type(ba2).__name__, ba2.upper(), ba2.startswith(B(b"ab")), ba2.decode("ascii"), ba2.hex())
print("2_ba_join", BA(b"-").join([B(b"x"), BA(b"y")]))
m_ba = ba2.upper
print("2_ba_captured", m_ba(), bytearray.upper(ba2))
ba2_pop = ba2.pop()
ba2.reverse()
ba2_cp = ba2.copy()
ba2.clear()
print("2_ba_ops", ba2_pop, bytes(ba2_cp), type(ba2_cp).__name__, len(ba2))

l2 = L([3, 1, 2])
l2.append(4)
l2.sort()
print("2_list", list(l2), l2.count(2), l2.index(3), l2.pop(), type(l2.copy()).__name__)
l2.reverse()
print("2_list_rev", list(l2), list.count(l2, 2))

print("2_tuple", t.count(2), t.index(3), tuple.count(t, 2))

d2 = D({"x": 10})
d2.update({"y": 20})
print("2_dict", d2.get("x"), d2.setdefault("z", 30), sorted(d2.keys()), sorted(d2.values()), sorted(d2.items()))
print("2_dict_pop_copy", d2.pop("y"), type(d2.copy()).__name__, type(d2.fromkeys(["k"], 1)).__name__)

st2 = St([1, 2])
st2.add(3)
st2.discard(1)
print("2_set", sorted(st2), sorted(st2.union([4])), sorted(st2.intersection([2, 4])), sorted(st2.difference([2])))
print("2_set_rel", st2.issubset([2, 3, 4]), st2.issuperset([2]), type(st2.copy()).__name__)

print("2_fs", sorted(fs.union([3])), sorted(fs.intersection([2, 3])), sorted(fs.difference([1])), sorted(fs.symmetric_difference([2, 3])))
print("2_fs_rel", fs.issubset([1, 2, 3]), fs.issuperset([1]), fs.isdisjoint([9]), type(fs.copy()).__name__)
m_fs = fs.union
print("2_fs_captured", sorted(m_fs([3])), sorted(frozenset.union(fs, [3])))

# --- 3. Iteration (iter, for, reversed, unpacking) ---
print("3_iter_seq", list(iter(s)), list(iter(b)), list(iter(ba)), list(iter(l)), list(iter(t)), list(iter(d)))
print("3_iter_sets", sorted(iter(st)), sorted(iter(fs)))
print("3_for_ba", [x for x in BA(b"xyz")])
print("3_for_fs", sorted([x for x in FS([3, 1, 2])]))
print("3_reversed", list(reversed(s)), list(reversed(b)), list(reversed(ba)), list(reversed(l)), list(reversed(t)), list(reversed(d)))
print("3_unpack", [*L([1, 2]), *T((3,)), *BA(b"A"), *sorted(FS([9]))], {**D({"k": 1}), "m": 2})

# --- 4. repr, str, format, unbound __repr__ / __str__ ---
print("4_repr_num", repr(i), repr(f), repr(c))
print("4_repr_seq", repr(s), repr(b), repr(ba), repr(l), repr(t))
print("4_repr_map", repr(d), repr(St([1])), repr(FS([1])))
print("4_str_num", str(i), str(f), str(c))
print("4_str_seq", str(s), str(b), str(ba), str(l), str(t))
print("4_str_map", str(d), str(St([1])), str(FS([1])))
print("4_unbound_repr", int.__repr__(i), float.__repr__(f), complex.__repr__(c), str.__repr__(s), bytes.__repr__(b), bytearray.__repr__(ba))
print("4_unbound_repr2", list.__repr__(l), tuple.__repr__(t), dict.__repr__(d), set.__repr__(St([1])), frozenset.__repr__(FS([1])))
print("4_format", format(i, "05d"), format(f, ".2f"), format(c, ""), format(s, ">7"))

# --- 5. Binary, reflected, unary, and in-place operators ---
print("5_int_ops", I(7) + 3, 3 + I(7), I(7) + I(3), I(7) * 2, I(7) // 2, I(7) % 2, I(2) ** 4, -I(7), +I(7), ~I(7), abs(I(-7)), divmod(I(7), 3), pow(I(2), 5, 7))
print("5_int_pow3", pow(2, I(5), 7), pow(2, 5, I(7)), pow(I(2), I(5), I(7)), int.__pow__(I(2), 5, 7), (2).__pow__(5, 7))
print("5_int_bit", I(6) | 1, I(6) & 3, I(6) ^ 3, I(3) << 2, I(12) >> 1)
print("5_float_ops", F(2.5) + 1, 1 + F(2.5), F(2.5) + I(2), F(2.5) * 2, -F(2.5), +F(2.5), abs(F(-2.5)), round(F(3.6)))
print("5_complex_ops", C(1, 2) + 3, 3 + C(1, 2), C(1, 2) + F(0.5), C(1, 2) + C(3, 4), C(1, 2) * 2, -C(1, 2), +C(1, 2), abs(C(3, 4)))
print("5_str_ops", S("a") + "b", "a" + S("b"), S("a") + S("b"), S("ab") * 2, 2 * S("ab"), S("%s-%d") % ("x", 1))
print("5_bytes_ops", B(b"a") + b"b", b"a" + B(b"b"), B(b"a") + B(b"b"), B(b"a") + BA(b"b"), BA(b"a") + B(b"b"), BA(b"a") + BA(b"b"), BA(b"a") + b"b", b"a" + BA(b"b"))
print("5_bytes_mul", B(b"ab") * 2, 2 * B(b"ab"), BA(b"ab") * 2, 2 * BA(b"ab"))
ba_ip = BA(b"a")
ba_orig = ba_ip
ba_ip += b"b"
ba_ip += B(b"c")
ba_ip += BA(b"d")
ba_ip *= 2
print("5_ba_inplace", bytes(ba_ip), ba_ip is ba_orig, type(ba_ip).__name__)

l_ip = L([1])
l_orig = l_ip
l_ip += [2]
l_ip += L([3])
l_ip *= 2
print("5_list_ops", L([1]) + [2], [0] + L([1]), L([1]) * 2, 2 * L([1]), list(l_ip), l_ip is l_orig, type(l_ip).__name__)
print("5_tuple_ops", T((1,)) + (2,), (0,) + T((1,)), T((1,)) * 2, 2 * T((1,)))

d_ip = D({"a": 1})
d_orig = d_ip
d_ip |= {"b": 2}
print("5_dict_ops", sorted((D({"a": 1}) | {"b": 2}).items()), sorted(({"a": 0} | D({"a": 1, "b": 2})).items()), sorted(d_ip.items()), d_ip is d_orig)

st_ip = St([1])
st_orig = st_ip
st_ip |= {2}
print("5_set_ops", sorted(St([1]) | {2}), sorted({0} | St([1])), sorted(St([1, 2]) & {2, 3}), sorted(St([1, 2]) - {2}), sorted(St([1, 2]) ^ {2, 3}), sorted(st_ip), st_ip is st_orig)
print("5_fs_ops", sorted(FS([1]) | {2}), sorted({0} | FS([1])), sorted(FS([1]) | FS([2])), sorted(FS([1, 2]) & {2, 3}), sorted(FS([1, 2]) - {2}), sorted(FS([1, 2]) ^ {2, 3}))

# --- 6. Equality and ordering ---
print("6_eq_num", I(1) == 1, I(1) == F(1.0), F(1.0) == C(1, 0), I(1) == C(1, 0), I(1) != 2)
print("6_eq_bytes", B(b"ab") == b"ab", B(b"ab") == BA(b"ab"), BA(b"ab") == bytearray(b"ab"), BA(b"ab") == b"ab", BA(b"ab") == BA(b"ab"))
print("6_eq_containers", L([1, 2]) == [1, 2], T((1, 2)) == (1, 2), D({"a": 1}) == {"a": 1}, St([1]) == {1}, St([1]) == FS([1]), FS([1]) == frozenset([1]))
print("6_ord", I(1) < I(2), F(1.0) < F(2.0), S("a") < S("b"), B(b"a") < B(b"b"), BA(b"a") < BA(b"b"), BA(b"a") < b"b", b"a" < BA(b"b"), L([1]) < L([2]), T((1,)) < T((2,)), St([1]) < St([1, 2]), FS([1]) < FS([1, 2]))

# --- 7. Membership (in / not in) ---
print("7_in", "el" in s, S("el") in s, 104 in b, b"el" in b, B(b"el") in b, 104 in ba, b"el" in ba, B(b"el") in ba, BA(b"el") in ba)
print("7_in2", 2 in l, 2 in t, "a" in d, S("a") in d, 2 in st, 2 in fs, 99 not in ba, 99 not in fs)

# --- 8. Indexing, slicing, item assignment/deletion ---
print("8_idx", s[1], s[1:4], b[0], b[1:4], ba[0], ba[1:4], l[1], l[0:2], t[1], t[0:2], d["a"])
ba_m = BA(b"abcd")
ba_m[0] = 65
ba_m[1:3] = B(b"XY")
del ba_m[3]
print("8_ba_mut", bytes(ba_m))
l_m = L([10, 20, 30])
l_m[1] = 99
del l_m[0]
print("8_list_mut", list(l_m))
d_m = D({"a": 1})
d_m["b"] = 2
del d_m["a"]
print("8_dict_mut", dict(d_m))

# --- 9. len, bool, hash, unbound __len__ ---
print("9_len", len(s), len(b), len(ba), len(l), len(t), len(d), len(st), len(fs))
print("9_len_empty", len(S("")), len(B(b"")), len(BA()), len(L()), len(T()), len(D()), len(St()), len(FS()))
print("9_unbound_len", str.__len__(s), bytes.__len__(b), bytearray.__len__(ba), list.__len__(l), tuple.__len__(t), dict.__len__(d), set.__len__(st), frozenset.__len__(fs))
print("9_bool_true", [bool(x) for x in all_objs])
print("9_bool_false", [bool(x) for x in [I(0), F(0.0), C(0, 0), S(""), B(b""), BA(), L(), T(), D(), St(), FS()]])

hashable_pairs = [(i, 42), (f, 3.5), (c, 3 + 4j), (s, "hello"), (b, b"hello"), (t, (1, 2, 3)), (fs, frozenset([1, 2]))]
print("9_hash_eq", [hash(sub) == hash(base) for sub, base in hashable_pairs])

unhashable_errs = []
for obj in [l, d, st, ba]:
    try:
        hash(obj)
        unhashable_errs.append("NO_ERR")
    except TypeError:
        unhashable_errs.append("TypeError")
print("9_unhashable", unhashable_errs)

# --- 10. Dict/set key interop & conversions ---
key_map = {I(10): "int", F(2.5): "flt", C(1, 2): "cplx", S("k"): "str", B(b"k"): "bytes", T((1, 2)): "tup", FS([7, 8]): "fs"}
print("10_key_interop", key_map[10], key_map[2.5], key_map[1 + 2j], key_map["k"], key_map[b"k"], key_map[(1, 2)], key_map[frozenset([7, 8])])
print("10_conversions", int(i), float(f), complex(c), str(s), bytes(b), bytes(ba), bytearray(ba), list(l), tuple(t), dict(d), sorted(set(st)), sorted(frozenset(fs)))

# --- 11. User dunder overrides take precedence over backing ---
class CustomInt(int):
    def __repr__(self):
        return f"CustomInt<{int.__repr__(self)}>"
    def __str__(self):
        return f"CI:{int.__str__(self)}"
    def __abs__(self):
        return 999
    def __add__(self, other):
        return ("add", int(self), other)
    def __eq__(self, other):
        return other == "magic"
    def __hash__(self):
        return 12345
    def __bool__(self):
        return False
    def __int__(self):
        return 777


ci = CustomInt(-5)
print("11_ci", repr(ci), str(ci), abs(ci), ci + 1, ci == "magic", ci == -5, hash(ci), bool(ci), int(ci))


class CustomFloat(float):
    def __float__(self):
        return 99.5
    def __abs__(self):
        return 88.5


cf = CustomFloat(-1.5)
print("11_cf", float(cf), abs(cf))


class CustomComplex(complex):
    def __complex__(self):
        return 7 + 8j
    def __abs__(self):
        return 55.0


cc = CustomComplex(3, 4)
print("11_cc", complex(cc), abs(cc))


class CustomBA(bytearray):
    def __len__(self):
        return 99
    def __repr__(self):
        return "CustomBA!"
    def __iter__(self):
        return iter([1, 2])
    def __contains__(self, item):
        return item == "yes"
    def __getitem__(self, item):
        return ("got", item)


cba = CustomBA(b"abc")
print("11_cba", len(cba), repr(cba), list(cba), "yes" in cba, 97 in cba, cba[0])


class CustomList(list):
    def __len__(self):
        return 42
    def __iter__(self):
        return iter(["x", "y"])
    def __repr__(self):
        return "CustomList!"


cl = CustomList([1, 2, 3])
print("11_cl", len(cl), list(cl), repr(cl))


class Forged:
    def __init__(self):
        self.__builtin_data__ = [1, 2, 3]


fg = Forged()
try:
    len(fg)
    print("11_forged_len", "UNEXPECTED")
except TypeError:
    print("11_forged_len", "TypeError")
print("11_forged_iter", hasattr(fg, "__iter__"))
