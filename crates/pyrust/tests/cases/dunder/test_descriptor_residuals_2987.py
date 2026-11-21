# Parity fixture for issue #2987 — descriptor residuals from PR #2984:
#   1. Metaclass __instancecheck__ / __subclasscheck__ descriptor binding
#   2. __del__ as a descriptor (property, user descriptor, cached_property,
#      staticmethod, classmethod, callable instance, dynamic mutation, inheritance)
#   3. functools.cached_property __set_name__, attrname, and error contracts
#   4. copy.copy class-level __copy__ lookup (ignoring instance __copy__,
#      class-level descriptor semantics) & copy.deepcopy instance lookup

import copy
import functools


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__} {exc.args!r}")


# =============================================================================
# 1. Metaclass __instancecheck__ / __subclasscheck__ descriptor binding
# =============================================================================

class MetaProp(type):
    @property
    def __instancecheck__(cls):
        return lambda inst: getattr(inst, "ok", False)

    @property
    def __subclasscheck__(cls):
        return lambda sub: getattr(sub, "sub_ok", False)


class CProp(metaclass=MetaProp):
    pass


class InstOk:
    ok = True


class InstNo:
    ok = False


class SubOk:
    sub_ok = True


class SubNo:
    sub_ok = False


print("1.prop isinstance:", isinstance(InstOk(), CProp), isinstance(InstNo(), CProp))
print("1.prop isinstance tuple:", isinstance(InstOk(), (int, CProp)), isinstance(InstNo(), (int, CProp)))
print("1.prop issubclass:", issubclass(SubOk, CProp), issubclass(SubNo, CProp))
print("1.prop issubclass tuple:", issubclass(SubOk, (int, CProp)), issubclass(SubNo, (int, CProp)))


class CheckDesc:
    def __init__(self, tag):
        self.tag = tag

    def __get__(self, obj, owner=None):
        if obj is None:
            return self
        return lambda target: getattr(target, self.tag, False)


class MetaUserDesc(type):
    __instancecheck__ = CheckDesc("ok")
    __subclasscheck__ = CheckDesc("sub_ok")


class CUserDesc(metaclass=MetaUserDesc):
    pass


print("1.desc isinstance:", isinstance(InstOk(), CUserDesc), isinstance(InstNo(), CUserDesc))
print("1.desc issubclass:", issubclass(SubOk, CUserDesc), issubclass(SubNo, CUserDesc))


class MetaStaticCM(type):
    @staticmethod
    def __instancecheck__(inst):
        return inst == "magic"

    @classmethod
    def __subclasscheck__(mcls, sub):
        return mcls is MetaStaticCM and sub is str


class CStaticCM(metaclass=MetaStaticCM):
    pass


print("1.static isinstance:", isinstance("magic", CStaticCM), isinstance("nope", CStaticCM))
print("1.cm issubclass:", issubclass(str, CStaticCM), issubclass(int, CStaticCM))


class CallChecker:
    def __call__(self, x):
        return x == 99 or x is float


class MetaCallInst(type):
    __instancecheck__ = CallChecker()
    __subclasscheck__ = CallChecker()


class CCallInst(metaclass=MetaCallInst):
    pass


print("1.callinst isinstance:", isinstance(99, CCallInst), isinstance(1, CCallInst))
print("1.callinst issubclass:", issubclass(float, CCallInst), issubclass(int, CCallInst))


class MetaNonCall(type):
    __instancecheck__ = 123
    __subclasscheck__ = 456


class CNonCall(metaclass=MetaNonCall):
    pass


show("1.noncall isinstance", lambda: isinstance(1, CNonCall))
show("1.noncall issubclass", lambda: issubclass(int, CNonCall))


class MetaNone(type):
    __instancecheck__ = None
    __subclasscheck__ = None


class CNone(metaclass=MetaNone):
    pass


show("1.none isinstance", lambda: isinstance(1, CNone))
show("1.none issubclass", lambda: issubclass(int, CNone))


class BadDesc:
    def __get__(self, obj, owner=None):
        raise RuntimeError("check_desc_boom")


class MetaBoom(type):
    __instancecheck__ = BadDesc()
    __subclasscheck__ = BadDesc()


class CBoom(metaclass=MetaBoom):
    pass


show("1.boom isinstance", lambda: isinstance(1, CBoom))
show("1.boom issubclass", lambda: issubclass(int, CBoom))


# =============================================================================
# 2. __del__ as a descriptor
# =============================================================================

del_log = []


class DelProp:
    @property
    def __del__(self):
        del_log.append("prop_get")
        return lambda: del_log.append("prop_ran")


def scope_del_prop():
    x = DelProp()
    y = x
    del x  # alias still alive: must NOT fire
    del y  # last binding: fires descriptor __del__


scope_del_prop()


class DelUserDesc:
    class D:
        def __get__(self, obj, owner=None):
            del_log.append(("desc_get", owner.__name__))
            return lambda: del_log.append("desc_ran")

    __del__ = D()


def scope_del_desc():
    x = DelUserDesc()
    del x


scope_del_desc()


class DelCachedProp:
    @functools.cached_property
    def __del__(self):
        del_log.append("cp_get")
        return lambda: del_log.append("cp_ran")


def scope_del_cp():
    x = DelCachedProp()
    del x


scope_del_cp()


class DelStatic:
    @staticmethod
    def __del__():
        del_log.append("static_ran")


def scope_del_static():
    x = DelStatic()
    del x


scope_del_static()


class DelClassMethod:
    @classmethod
    def __del__(cls):
        del_log.append(("cm_ran", cls.__name__))


def scope_del_cm():
    x = DelClassMethod()
    del x


scope_del_cm()


class DelCallInst:
    class CB:
        def __call__(self):
            del_log.append("callinst_ran")

    __del__ = CB()


def scope_del_ci():
    x = DelCallInst()
    del x


scope_del_ci()


# Dynamic mutation: add descriptor __del__ after class creation, then remove it
class DelDynamicBase:
    pass


class DelDynamicSub(DelDynamicBase):
    pass


def scope_del_dyn():
    a = DelDynamicBase()
    b = DelDynamicSub()
    del a
    del b


scope_del_dyn()  # no __del__ yet -> populates negative cache
DelDynamicBase.__del__ = property(lambda self: lambda: del_log.append(("dyn", type(self).__name__)))
scope_del_dyn()  # base mutation invalidates both base & subclass cache
del DelDynamicBase.__del__
scope_del_dyn()  # deleted -> no longer fires

print("2.del_log:", del_log)


# =============================================================================
# 3. functools.cached_property __set_name__ & error contracts
# =============================================================================

cp_raw = functools.cached_property(lambda self: 42)
print("3.raw attrname:", cp_raw.attrname)


class DummyCP:
    pass


show("3.raw __get__ without __set_name__", lambda: cp_raw.__get__(DummyCP(), DummyCP))
print("3.raw __get__ on class is self:", cp_raw.__get__(None, DummyCP) is cp_raw)
cp_raw.__set_name__(DummyCP, "val")
print("3.after __set_name__:", cp_raw.attrname)
cp_raw.__set_name__(DummyCP, "val")  # same name is a no-op
print("3.after same __set_name__:", cp_raw.attrname)
show("3.different __set_name__", lambda: cp_raw.__set_name__(DummyCP, "other"))


class CPInClass:
    def __init__(self):
        self.calls = 0

    @functools.cached_property
    def x(self):
        self.calls += 1
        return 99


print("3.class dict attrname:", CPInClass.__dict__["x"].attrname)
print("3.class access is descriptor:", CPInClass.x is CPInClass.__dict__["x"])
print("3.type name:", type(CPInClass.x).__name__)
print("3.func name:", CPInClass.x.func.__name__)
cp_inst = CPInClass()
print("3.inst access:", cp_inst.x, cp_inst.x, cp_inst.calls, cp_inst.__dict__["x"])


# Same cached_property bound to two different names in a class body
shared_cp = functools.cached_property(lambda self: 1)
try:
    class BadReuse:
        a = shared_cp
        b = shared_cp
except TypeError as exc:
    print("3.reuse in class body:", type(exc).__name__, exc.args)


# __slots__ without __dict__ raises exact CPython TypeError
class CPSlots:
    __slots__ = ()

    @functools.cached_property
    def x(self):
        return 1


show("3.slots access", lambda: CPSlots().x)


# cached_property in a dunder slot uses its __set_name__ attrname
class CPDunder:
    def __init__(self):
        self.calls = 0

    @functools.cached_property
    def __len__(self):
        self.calls += 1
        return lambda: 7


cpd = CPDunder()
print("3.dunder attrname:", CPDunder.__dict__["__len__"].attrname)
print("3.dunder len:", len(cpd), len(cpd), cpd.calls, "__len__" in cpd.__dict__)


# =============================================================================
# 4. copy.copy class-level lookup & copy.deepcopy instance lookup
# =============================================================================

class CopyInstOnly:
    def __init__(self):
        self.v = 10
        self.__copy__ = lambda: "INSTANCE_COPY"
        self.__deepcopy__ = lambda memo: "INSTANCE_DEEPCOPY"


cio = CopyInstOnly()
c_cio = copy.copy(cio)
dc_cio = copy.deepcopy(cio)
print("4.copy ignores instance __copy__:", isinstance(c_cio, CopyInstOnly), c_cio is not cio, c_cio.v)
print("4.deepcopy uses instance __deepcopy__:", dc_cio)


class CopyPlain:
    def __copy__(self):
        return "PLAIN_COPY"


print("4.copy plain:", copy.copy(CopyPlain()))


class CopyProp:
    @property
    def __copy__(self):
        return lambda: "PROP_COPY"


show("4.copy class property", lambda: copy.copy(CopyProp()))


class CopyCP:
    @functools.cached_property
    def __copy__(self):
        return lambda: "CP_COPY"


show("4.copy class cached_property", lambda: copy.copy(CopyCP()))


class CopyDesc:
    def __get__(self, obj, owner=None):
        return lambda x: ("DESC", obj is None, owner.__name__, type(x).__name__)


class CCopyDesc:
    __copy__ = CopyDesc()


print("4.copy class user desc:", copy.copy(CCopyDesc()))


class CopyStatic:
    @staticmethod
    def __copy__(x):
        return ("STATIC", type(x).__name__)


print("4.copy staticmethod:", copy.copy(CopyStatic()))


class CopyCM:
    @classmethod
    def __copy__(cls, x):
        return ("CM", cls.__name__, type(x).__name__)


print("4.copy classmethod:", copy.copy(CopyCM()))


class CopyCallInst:
    class CB:
        def __call__(self, x):
            return ("CALLINST", type(x).__name__)

    __copy__ = CB()


print("4.copy callable inst:", copy.copy(CopyCallInst()))


class CopyNonCall:
    __copy__ = 123


show("4.copy noncall", lambda: copy.copy(CopyNonCall()))


class CopyNone:
    __copy__ = None

    def __init__(self):
        self.v = 5


cn = CopyNone()
cn_c = copy.copy(cn)
print("4.copy None fallback:", isinstance(cn_c, CopyNone), cn_c is not cn, cn_c.v)


class CopyMeta(type):
    @property
    def __copy__(cls):
        return lambda x: ("META_PROP", cls.__name__, type(x).__name__)


class CCopyMeta(metaclass=CopyMeta):
    pass


print("4.copy metaclass property:", copy.copy(CCopyMeta()))


class DeepCopyProp:
    @property
    def __deepcopy__(self):
        return lambda memo: ("DC_PROP", isinstance(memo, dict))


print("4.deepcopy property:", copy.deepcopy(DeepCopyProp()))


class DeepCopyNone:
    __deepcopy__ = None

    def __init__(self):
        self.v = [1, 2]


dcn = DeepCopyNone()
dcn_c = copy.deepcopy(dcn)
print("4.deepcopy None fallback:", isinstance(dcn_c, DeepCopyNone), dcn_c is not dcn, dcn_c.v, dcn_c.v is not dcn.v)
