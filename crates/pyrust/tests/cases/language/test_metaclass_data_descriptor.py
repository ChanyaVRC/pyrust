# Parity fixture for issue #2081: a data descriptor on the metaclass shadows
# a same-named attribute in the class's own dict, matching CPython's
# type.__getattribute__ priority (metatype data descriptor > class dict >
# metatype non-data descriptor/value).

# --- property (data descriptor) on the metaclass wins over class-own attr ---
class Meta(type):
    @property
    def x(cls):
        return "from-metaclass-property"

class C(metaclass=Meta):
    x = "class-own"

print(C.x)                      # from-metaclass-property

# --- Non-data metaclass attribute does NOT shadow class-own (unchanged) ---
class Meta2(type):
    y = "from-metaclass"

class C2(metaclass=Meta2):
    y = "class-own-2"

print(C2.y)                     # class-own-2

# --- User-defined data descriptor (__get__ + __set__) on metaclass wins ---
class DataDesc:
    def __get__(self, obj, owner):
        return "data-desc-get"
    def __set__(self, obj, value):
        pass

class Meta3(type):
    d = DataDesc()

class C3(metaclass=Meta3):
    d = "class-own-3"

print(C3.d)                     # data-desc-get

# --- Non-data descriptor (__get__ only) on metaclass does NOT shadow ---
class NonDataDesc:
    def __get__(self, obj, owner):
        return "non-data-get"

class Meta4(type):
    nd = NonDataDesc()

class C4(metaclass=Meta4):
    nd = "class-own-4"

print(C4.nd)                    # class-own-4

# --- Metaclass property the class does NOT override is still reachable ---
class Meta5(type):
    @property
    def only_meta(cls):
        return "only-on-metaclass"

class C5(metaclass=Meta5):
    pass

print(C5.only_meta)             # only-on-metaclass

# --- Instance access is unaffected by a metaclass data descriptor ---
class Meta6(type):
    @property
    def shared(cls):
        return "meta-property"

class C6(metaclass=Meta6):
    shared = "class-attr"

print(C6.shared)                # meta-property
print(C6().shared)              # class-attr  (instance reads the class dict)
