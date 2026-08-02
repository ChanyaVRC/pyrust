import operator


# Issue #2947: an implicit special-method lookup on a class resolves the slot
# on its metaclass.  A classmethod descriptor there binds type(receiver), which
# is the metaclass, rather than the receiver class itself.


events = []


class MetaAll(type):
    @classmethod
    def __len__(cls):
        events.append(("len", cls.__name__))
        return 4

    @classmethod
    def __getitem__(cls, key):
        events.append(("getitem", cls.__name__, key))
        return (cls.__name__, key)

    @classmethod
    def __iter__(cls):
        events.append(("iter", cls.__name__))
        return iter([cls.__name__])

    @classmethod
    def __contains__(cls, item):
        events.append(("contains", cls.__name__, item))
        return item == "needle"

    @classmethod
    def __call__(cls, value):
        events.append(("call", cls.__name__, value))
        return (cls.__name__, value)

    @classmethod
    def __instancecheck__(cls, instance):
        events.append(("instancecheck", cls.__name__, type(instance).__name__))
        return True

    @classmethod
    def __subclasscheck__(cls, subclass):
        events.append(("subclasscheck", cls.__name__, subclass.__name__))
        return True


class WithMeta(metaclass=MetaAll):
    pass


class Unrelated:
    pass


events.clear()
print("len", len(WithMeta), events)
events.clear()
print("getitem", WithMeta[5], events)
events.clear()
print("iter", next(iter(WithMeta)), events)
events.clear()
print("contains", "needle" in WithMeta, events)
events.clear()
print("call", WithMeta(7), events)
events.clear()
print("instancecheck", isinstance(Unrelated(), WithMeta), events)
events.clear()
print("subclasscheck", issubclass(Unrelated, WithMeta), events)


# Wrapper-valued classmethods bind to a callable adapter.  Gated metaclass
# hooks must admit that bound representation just like a bound user function.
class CallableCall:
    def __call__(self, cls, value):
        events.append(("wrapped call", cls.__name__, value))
        return (cls.__name__, value)


class CallableInstanceCheck:
    def __call__(self, cls, instance):
        events.append(("wrapped instancecheck", cls.__name__, type(instance).__name__))
        return True


class CallableSubclassCheck:
    def __call__(self, cls, subclass):
        events.append(("wrapped subclasscheck", cls.__name__, subclass.__name__))
        return True


class WrappedHooksMeta(type):
    __call__ = classmethod(CallableCall())
    __instancecheck__ = classmethod(CallableInstanceCheck())
    __subclasscheck__ = classmethod(CallableSubclassCheck())


class WithWrappedHooks(metaclass=WrappedHooksMeta):
    pass


events.clear()
try:
    print("wrapped call", WithWrappedHooks(4), events)
except TypeError as error:
    print("wrapped call error", str(error), events)
events.clear()
print("wrapped instancecheck", isinstance(Unrelated(), WithWrappedHooks), events)
events.clear()
print("wrapped subclasscheck", issubclass(Unrelated, WithWrappedHooks), events)


# The wrapper-value spelling must use the same binding owner as a decorated
# Python function.
class CallableLength:
    def __call__(self, cls):
        events.append(("wrapped len", cls.__name__))
        return 2


class WrappedMeta(type):
    __len__ = classmethod(CallableLength())


class WithWrappedMeta(metaclass=WrappedMeta):
    pass


events.clear()
print("wrapped", len(WithWrappedMeta), events)


# Shared value-protocol lookup must preserve the same metaclass owner.  These
# paths do not use the direct len/index/iteration dispatch above.
class ConversionMeta(type):
    @classmethod
    def __index__(cls):
        events.append(("index", cls.__name__))
        return 7

    @classmethod
    def __float__(cls):
        events.append(("float", cls.__name__))
        return 1.5

    @classmethod
    def __length_hint__(cls):
        events.append(("length hint", cls.__name__))
        return 8


class WithConversions(metaclass=ConversionMeta):
    pass


events.clear()
print("printf", "%.1f" % WithConversions, events)
events.clear()
print("length hint", operator.length_hint(WithConversions), events)
events.clear()
try:
    print("index", operator.index(WithConversions), events)
except TypeError as error:
    print("index error", str(error), events)


class CallableIndex:
    def __call__(self, cls):
        events.append(("wrapped index", cls.__name__))
        return 9


class WrappedConversionMeta(type):
    __index__ = classmethod(CallableIndex())


class WithWrappedConversion(metaclass=WrappedConversionMeta):
    pass


events.clear()
try:
    print("wrapped index", operator.index(WithWrappedConversion), events)
except TypeError as error:
    print("wrapped index error", str(error), events)


# The receiver class's own attribute cannot shadow the implicit metaclass
# slot used by PyNumber_Index/operator.index.
class ShadowIndexMeta(type):
    @classmethod
    def __index__(cls):
        events.append(("shadow meta index", cls.__name__))
        return 10


class WithShadowIndex(metaclass=ShadowIndexMeta):
    @classmethod
    def __index__(cls):
        events.append(("shadow class index", cls.__name__))
        return 11


events.clear()
print("shadow index", operator.index(WithShadowIndex), events)


# A regular metaclass function is unbound when read from the metaclass class.
# operator.index must bind it to the receiver class before calling it.  The
# corresponding staticmethod remains a zero-argument callable.
class RegularIndexMeta(type):
    def __index__(self):
        events.append(("regular index", self.__name__))
        return 12


class WithRegularIndex(metaclass=RegularIndexMeta):
    pass


events.clear()
try:
    print("regular index", operator.index(WithRegularIndex), events)
except TypeError as error:
    print("regular index error", str(error), events)


class StaticIndexMeta(type):
    @staticmethod
    def __index__():
        events.append(("static index",))
        return 13


class WithStaticIndex(metaclass=StaticIndexMeta):
    pass


events.clear()
print("static index", operator.index(WithStaticIndex), events)


class IndexDescriptor:
    def __get__(self, instance, owner):
        events.append(("index descriptor", instance.__name__, owner.__name__))
        return lambda: 14


class DescriptorIndexMeta(type):
    __index__ = IndexDescriptor()


class WithDescriptorIndex(metaclass=DescriptorIndexMeta):
    pass


events.clear()
print("descriptor index", operator.index(WithDescriptorIndex), events)


# Descriptor capability comes from type(slot), never an instance attribute
# named __get__.  This callable is therefore invoked directly.
class CallableIndexSlot:
    def __call__(self):
        events.append(("callable index slot",))
        return 15


callable_index_slot = CallableIndexSlot()


def fake_index_get(instance, owner):
    events.append(("fake index get", instance.__name__, owner.__name__))
    return lambda: 16


callable_index_slot.__get__ = fake_index_get


class InstanceGetIndexMeta(type):
    __index__ = callable_index_slot


class WithInstanceGetIndex(metaclass=InstanceGetIndexMeta):
    pass


events.clear()
print("instance get index", operator.index(WithInstanceGetIndex), events)


# An instance attribute also cannot shadow a real __get__ supplied by the
# descriptor's class.
class ShadowedGetIndexDescriptor:
    def __get__(self, instance, owner):
        events.append(("class index get", instance.__name__, owner.__name__))
        return lambda: 17


shadowed_get_index = ShadowedGetIndexDescriptor()


def shadow_index_get(instance, owner):
    events.append(("shadow index get", instance.__name__, owner.__name__))
    return lambda: 18


shadowed_get_index.__get__ = shadow_index_get


class ShadowedGetIndexMeta(type):
    __index__ = shadowed_get_index


class WithShadowedGetIndex(metaclass=ShadowedGetIndexMeta):
    pass


events.clear()
print("shadowed get index", operator.index(WithShadowedGetIndex), events)


# slot_tp_descr_get invokes the raw __get__ entry without descriptor-binding
# __get__ itself.  A raw classmethod is not callable, while a raw staticmethod
# still receives all three positional arguments.
class ClassGetIndexDescriptor:
    @classmethod
    def __get__(cls, instance, owner):
        events.append(("class get body",))
        return lambda: 19


class ClassGetIndexMeta(type):
    __index__ = ClassGetIndexDescriptor()


class WithClassGetIndex(metaclass=ClassGetIndexMeta):
    pass


events.clear()
try:
    print("class get index", operator.index(WithClassGetIndex), events)
except TypeError as error:
    print("class get index error", str(error), events)


class StaticGetIndexDescriptor:
    @staticmethod
    def __get__(instance, owner):
        events.append(("static get body", instance.__name__, owner.__name__))
        return lambda: 20


class StaticGetIndexMeta(type):
    __index__ = StaticGetIndexDescriptor()


class WithStaticGetIndex(metaclass=StaticGetIndexMeta):
    pass


events.clear()
try:
    print("static get index", operator.index(WithStaticGetIndex), events)
except TypeError as error:
    print("static get index error", str(error), events)


# An inherited slot binds the receiver's actual metaclass, not the metaclass
# that originally defined the descriptor.
class ParentMeta(type):
    @classmethod
    def __getitem__(cls, key):
        return (cls.__name__, key)


class ChildMeta(ParentMeta):
    pass


class WithChildMeta(metaclass=ChildMeta):
    pass


print("inherited", WithChildMeta[9])


# Ordinary instance special methods keep binding type(instance).
class InstanceBase:
    @classmethod
    def __len__(cls):
        events.append(("instance len", cls.__name__))
        return 3


class InstanceSub(InstanceBase):
    pass


events.clear()
print("instance", len(InstanceSub()), events)


# operator.index must also preserve ordinary instance classmethod binding.  A
# type-level lookup is already bound to type(instance), so passing the instance
# again would double-bind both decorator spellings.
class InstanceIndexBase:
    @classmethod
    def __index__(cls):
        events.append(("instance index", cls.__name__))
        return 21


class InstanceIndexSub(InstanceIndexBase):
    pass


events.clear()
try:
    print("instance index", operator.index(InstanceIndexSub()), events)
except TypeError as error:
    print("instance index error", str(error), events)


class CallableInstanceIndex:
    def __call__(self, cls):
        events.append(("wrapped instance index", cls.__name__))
        return 22


class WrappedInstanceIndex:
    __index__ = classmethod(CallableInstanceIndex())


events.clear()
try:
    print("wrapped instance index", operator.index(WrappedInstanceIndex()), events)
except TypeError as error:
    print("wrapped instance index error", str(error), events)


# Regular and staticmethod metaclass slots keep their own descriptor rules.
class RegularMeta(type):
    def __len__(self):
        events.append(("regular len", self.__name__))
        return 5


class WithRegularMeta(metaclass=RegularMeta):
    pass


events.clear()
print("regular", len(WithRegularMeta), events)


class StaticMeta(type):
    @staticmethod
    def __len__():
        events.append(("static len",))
        return 6


class WithStaticMeta(metaclass=StaticMeta):
    pass


events.clear()
print("static", len(WithStaticMeta), events)


# These class-construction/class-binding hooks deliberately receive their
# existing class owner.  A global change to PyClass classmethod binding would
# incorrectly replace PrepareMeta with type here.
hook_events = []


class InitBase:
    @classmethod
    def __init_subclass__(cls, **kwargs):
        hook_events.append(("init subclass", cls.__name__))


class InitChild(InitBase):
    pass


class ClassGetItem:
    def __class_getitem__(cls, key):
        return (cls.__name__, key)


class PrepareMeta(type):
    @classmethod
    def __prepare__(cls, name, bases):
        hook_events.append(("prepare", cls.__name__, name))
        return {}


class Prepared(metaclass=PrepareMeta):
    pass


print("hooks", hook_events, ClassGetItem[1])
