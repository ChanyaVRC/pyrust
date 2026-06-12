def t(label, fn):
    try: print(label, "=", repr(fn())[:60])
    except Exception as e: print(label, "!", type(e).__name__, str(e)[:70])
class L(list): pass
class T(tuple): pass
class S(str): pass
class BA(bytearray): pass
class LR(list):
    def __repr__(self): return "CUSTOM"
class LH(list):
    def __hash__(self): return 5
# KeyError message embeds key repr
t("ke", lambda: {LH([1]): 1}[LH([2])])
t("ke-str", lambda: {}.__getitem__(S("k")))
# ValueError from list.remove embeds value repr
t("remove", lambda: [1, 2].remove(L([9])))
# tuple.index
t("tindex", lambda: (1, 2).index(T((9,))))
# repr direct (interpreter side, sanity)
t("repr-L", lambda: repr(L([1]))); t("repr-LR", lambda: repr(LR([1]))); t("repr-BA", lambda: repr(BA(b"a")))
# override case in exception message: CPython calls the override; pyrust core can't — expect KNOWN divergence, check what it prints
class LRH(list):
    def __hash__(self): return 6
    def __repr__(self): return "OVR"
t("ke-ovr", lambda: {LRH([1]): 1}[LRH([2])])

# repr(e) / !r / multi-arg / non-KeyError str — all dispatch arg overrides.
class LO2(list):
    def __hash__(self): return 6
    def __repr__(self): return "OVR"
class LH3(list):
    def __hash__(self): return 5
try: {LO2([1]): 1}[LO2([2])]
except KeyError as e:
    print(repr(e), f"{e!r}", "{0!r}".format(e))
print(repr(ValueError(LH3([7]))), repr(ValueError(LO2([7]))))
print(str(ValueError(LO2([7]))), str(ValueError(LO2([1]), LH3([2]))))
print(repr(KeyError(LO2([1]), LH3([2]))))
