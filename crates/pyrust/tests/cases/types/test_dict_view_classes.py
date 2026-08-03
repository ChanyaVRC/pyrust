"""Issue #3007: dictionary views are real, final classes with ABC surfaces."""

import builtins
import collections.abc as abc
from collections import OrderedDict


SET_DUNDERS = (
    "__and__",
    "__rand__",
    "__or__",
    "__ror__",
    "__sub__",
    "__rsub__",
    "__xor__",
    "__rxor__",
)
COMPARISON_DUNDERS = ("__eq__", "__ne__", "__le__", "__lt__", "__ge__", "__gt__")
SURFACE = SET_DUNDERS + ("__contains__", "__iter__", "__len__") + COMPARISON_DUNDERS
OWNED_SURFACE = SURFACE + ("__reversed__", "isdisjoint")
ABC_TYPES = (
    abc.MappingView,
    abc.KeysView,
    abc.ItemsView,
    abc.ValuesView,
    abc.Set,
    abc.Iterable,
    abc.Sized,
    abc.Container,
    abc.Reversible,
    abc.Hashable,
)
OBJECT_SLOT_NAMES = ("__repr__", "__str__", "__hash__", "__eq__", "__ne__")
ABC_CONTROL_NAMES = (
    "__instancecheck__",
    "__subclasscheck__",
    "__subclasshook__",
    "register",
)


def outcome(fn):
    try:
        return ("ok", fn())
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def subclass_outcome(cls):
    try:
        type("Child", (cls,), {})
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def hash_outcome(value):
    try:
        hash(value)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def object_new_outcome(cls):
    try:
        cls.__new__(cls)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def explicit_object_new_outcome(cls):
    try:
        object.__new__(cls)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def type_call_outcome(cls):
    try:
        type.__call__(cls)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def class_shape(label, view, fresh):
    cls = type(view)
    class_dict = getattr(cls, "__dict__", {})
    print(
        "class",
        label,
        repr(cls),
        getattr(cls, "__name__", None),
        getattr(cls, "__module__", None),
        cls is type(fresh),
        outcome(lambda: issubclass(cls, object)),
    )
    print("surface", label, tuple(hasattr(view, name) for name in SURFACE))
    print("owned", label, tuple(name in class_dict for name in OWNED_SURFACE))
    print(
        "hierarchy",
        label,
        tuple(base.__name__ for base in cls.__bases__),
        tuple(base.__name__ for base in cls.__mro__),
    )
    print(
        "abc-controls",
        label,
        tuple(hasattr(view, name) for name in ABC_CONTROL_NAMES),
        tuple(name in class_dict for name in ABC_CONTROL_NAMES),
    )
    print(
        "object-slots",
        label,
        tuple(name in class_dict for name in OBJECT_SLOT_NAMES),
        getattr(cls, "__hash__", "missing") is None,
    )
    print(
        "direct-text-hash",
        label,
        outcome(lambda: view.__repr__()),
        outcome(lambda: view.__str__()),
        hash_outcome(view),
    )
    print("abc", label, tuple(isinstance(view, cls) for cls in ABC_TYPES))
    print("abc-class", label, tuple(issubclass(type(view), cls) for cls in ABC_TYPES))
    print("construct", label, outcome(lambda: cls()))
    print("type-call", label, type_call_outcome(cls))
    print(
        "object-new",
        label,
        object_new_outcome(cls),
        explicit_object_new_outcome(cls),
    )
    print("subclass", label, subclass_outcome(cls))


def unbound_view_descriptors(keys_view, items_view, values_view):
    keys_cls = type(keys_view)
    items_cls = type(items_view)
    values_cls = type(values_view)
    print(
        "unbound-keys",
        outcome(lambda: keys_cls.__len__(keys_view)),
        outcome(lambda: list(keys_cls.__iter__(keys_view))),
        outcome(lambda: keys_cls.__contains__(keys_view, "a")),
        outcome(lambda: keys_cls.__eq__(keys_view, {"a", "b"})),
        outcome(lambda: keys_cls.__le__(keys_view, {"a", "b"})),
        outcome(lambda: keys_cls.isdisjoint(keys_view, {"missing"})),
        outcome(lambda: list(keys_cls.__reversed__(keys_view))),
    )
    print(
        "unbound-items",
        outcome(lambda: items_cls.__len__(items_view)),
        outcome(lambda: list(items_cls.__iter__(items_view))),
        outcome(lambda: items_cls.__contains__(items_view, ("a", 1))),
        outcome(
            lambda: items_cls.__eq__(items_view, {("a", 1), ("b", 2)})
        ),
        outcome(
            lambda: items_cls.__le__(items_view, {("a", 1), ("b", 2)})
        ),
        outcome(lambda: items_cls.isdisjoint(items_view, {("missing", 0)})),
        outcome(lambda: list(items_cls.__reversed__(items_view))),
    )
    print(
        "unbound-values",
        outcome(lambda: values_cls.__len__(values_view)),
        outcome(lambda: list(values_cls.__iter__(values_view))),
        outcome(lambda: values_cls.__eq__(values_view, values_view)),
        outcome(lambda: list(values_cls.__reversed__(values_view))),
    )
    print(
        "unbound-errors",
        outcome(lambda: keys_cls.__len__(items_view)),
        outcome(lambda: keys_cls.__len__()),
        outcome(lambda: keys_cls.__len__(keys_view, 1)),
        outcome(lambda: keys_cls.__len__(self=keys_view)),
    )


def immutable_delete_outcome(cls, name):
    try:
        delattr(cls, name)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def assign_attr_outcome(obj, name, value):
    try:
        setattr(obj, name, value)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def delete_attr_outcome(obj, name):
    try:
        delattr(obj, name)
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def assign_item_outcome(obj, key, value):
    try:
        obj[key] = value
        return "ok"
    except Exception as exc:
        return (type(exc).__name__, str(exc))


def default_error_shape(label, view):
    print(
        "view-default-errors",
        label,
        outcome(lambda: view[0]),
        assign_item_outcome(view, 0, None),
        outcome(lambda: view()),
        outcome(lambda: -view),
        outcome(lambda: view + ()),
    )
    print(
        "view-attribute-errors",
        label,
        outcome(lambda: getattr(view, "missing")),
        assign_attr_outcome(view, "missing", None),
        delete_attr_outcome(view, "missing"),
        assign_attr_outcome(view, "__iter__", None),
        delete_attr_outcome(view, "__iter__"),
    )


def stable_bound_repr(method):
    text = repr(method)
    marker = " at 0x"
    if marker in text:
        return text.split(marker, 1)[0] + " at <addr>>"
    return text


def bound_view_method_shape(label, view, names):
    print(
        "bound-view-methods",
        label,
        tuple(
            (
                name,
                type(getattr(view, name)).__name__,
                stable_bound_repr(getattr(view, name)),
                getattr(view, name).__name__,
                getattr(view, name).__qualname__,
            )
            for name in names
        ),
    )


def view_descriptor_keyword_errors(label, view):
    print(
        "view-keyword-reversed",
        label,
        outcome(lambda: view.__reversed__(unexpected=True)),
        outcome(lambda: type(view).__reversed__(view, unexpected=True)),
    )
    if hasattr(view, "isdisjoint"):
        print(
            "view-keyword-isdisjoint",
            label,
            outcome(lambda: view.isdisjoint(other=set())),
            outcome(lambda: type(view).isdisjoint(view, other=set())),
        )
        print(
            "view-direct-isdisjoint-arity",
            label,
            outcome(lambda: view.isdisjoint()),
            outcome(lambda: view.isdisjoint(set(), set())),
        )


def saved_isdisjoint_error_shape(label, view):
    method = view.isdisjoint
    print(
        "saved-isdisjoint-errors",
        label,
        outcome(lambda: method()),
        outcome(lambda: method(set(), set())),
        outcome(lambda: method(other=set())),
    )
    print(
        "expanded-isdisjoint-errors",
        label,
        outcome(lambda: view.isdisjoint(*())),
        outcome(lambda: view.isdisjoint(**{"other": set()})),
    )

    class Holder:
        pass

    holder = Holder()
    holder.f = method

    class ClassHolder:
        pass

    ClassHolder.f = method

    class PropertyHolder:
        pass

    PropertyHolder.f = property(lambda self: method)

    class CustomHolder:
        def __getattribute__(self, name):
            if name == "f":
                return method
            return object.__getattribute__(self, name)

    print(
        "stored-isdisjoint-errors",
        label,
        outcome(lambda: holder.f()),
        outcome(lambda: ClassHolder().f()),
        outcome(lambda: PropertyHolder().f()),
        outcome(lambda: CustomHolder().f()),
    )


def compiler_cutoff_isdisjoint_error_shape(label, view):
    cases = (
        ("pos-before", 29, 0),
        ("pos-after", 30, 0),
        ("kw-before", 0, 28),
        ("kw-after", 0, 29),
        ("mixed-before", 27, 1),
        ("mixed-after", 28, 1),
    )
    results = []
    for case, npos, nkw in cases:
        arguments = ["None"] * npos
        arguments.extend("k{}=None".format(index) for index in range(nkw))
        namespace = {}
        exec(
            "def call(view):\n    return view.isdisjoint({})".format(
                ", ".join(arguments)
            ),
            namespace,
        )
        results.append((case, outcome(lambda: namespace["call"](view))))
    print("compiler-isdisjoint-call-shape", label, tuple(results))


def mapping_descriptor_shape(label, mapping, view):
    cls = type(view)
    owner = cls.__mro__[1] if label.startswith("ordered-") else cls
    descriptor = owner.__dict__["mapping"]
    print(
        "class-dict-specials",
        label,
        tuple(
            name in cls.__dict__
            for name in ("mapping", "__getattribute__", "__doc__")
        ),
    )
    print(
        "mapping-descriptor",
        label,
        repr(descriptor),
        type(descriptor).__name__,
        descriptor.__name__,
        descriptor.__objclass__ is owner,
        descriptor.__doc__,
        tuple(
            hasattr(descriptor, name)
            for name in ("__get__", "__set__", "__delete__")
        ),
        outcome(lambda: descriptor.__get__(None)),
        descriptor.__get__(None, owner) is descriptor,
        outcome(lambda: dict(descriptor.__get__(view))),
        outcome(lambda: descriptor.__get__(1)),
    )
    print(
        "mapping-direct-mutation",
        label,
        outcome(lambda: descriptor.__set__(view, None)),
        outcome(lambda: descriptor.__set__(1, None)),
        outcome(lambda: descriptor.__delete__(view)),
        outcome(lambda: descriptor.__delete__(1)),
    )
    print(
        "mapping-protocol-errors",
        label,
        outcome(lambda: descriptor.__get__()),
        outcome(lambda: descriptor.__get__(view, owner, owner)),
        outcome(lambda: descriptor.__get__(object=view)),
        outcome(lambda: descriptor.__set__()),
        outcome(lambda: descriptor.__set__(object=view, value=None)),
        outcome(lambda: descriptor.__delete__()),
        outcome(lambda: descriptor.__delete__(object=view)),
    )
    print(
        "mapping-readonly",
        label,
        assign_attr_outcome(view, "mapping", None),
        delete_attr_outcome(view, "mapping"),
    )
    proxy = descriptor.__get__(view)
    mapping["live"] = 3
    print("mapping-live", label, tuple(proxy.items()))
    del mapping["live"]


def view_getattribute_shape(label, view):
    descriptor = type(view).__getattribute__
    print(
        "view-getattribute",
        label,
        repr(descriptor),
        type(descriptor).__name__,
        descriptor.__name__,
        descriptor.__objclass__.__name__,
        outcome(lambda: dict(descriptor(view, "mapping"))),
    )


def descriptor_inventory(label, cls, names):
    print(
        "descriptor-inventory",
        label,
        tuple(
            (
                name,
                type(getattr(cls, name)).__name__,
                repr(getattr(cls, name)),
                getattr(cls, name).__name__,
                getattr(cls, name).__objclass__.__name__,
            )
            for name in names
        ),
    )


def direct_set_dunders(label, view, other, exact, smaller, larger):
    print(
        "direct-set",
        label,
        outcome(lambda: sorted(view.__and__(other))),
        outcome(lambda: sorted(view.__rand__(other))),
        outcome(lambda: sorted(view.__or__(other))),
        outcome(lambda: sorted(view.__ror__(other))),
        outcome(lambda: sorted(view.__sub__(other))),
        outcome(lambda: sorted(view.__rsub__(other))),
        outcome(lambda: sorted(view.__xor__(other))),
        outcome(lambda: sorted(view.__rxor__(other))),
    )
    print(
        "direct-common",
        label,
        outcome(lambda: view.__len__()),
        outcome(lambda: list(view.__iter__())),
        outcome(lambda: view.__contains__(next(iter(exact)))),
        outcome(lambda: view.__eq__(exact)),
        outcome(lambda: view.__ne__(larger)),
        outcome(lambda: view.__le__(exact)),
        outcome(lambda: view.__lt__(larger)),
        outcome(lambda: view.__ge__(exact)),
        outcome(lambda: view.__gt__(smaller)),
    )


def native_set_ops(label, view, exact, smaller, larger):
    print(
        "native-set",
        label,
        sorted(view & larger),
        sorted(view | larger),
        sorted(view - smaller),
        sorted(view ^ larger),
        view <= exact,
        view < larger,
        view >= exact,
        view > smaller,
        view == exact,
        next(iter(exact)) in view,
        len(view),
        list(reversed(view)),
        view.isdisjoint({"missing"}),
    )


class NonSet:
    pass


class ReflectedIterable:
    def __init__(self, values):
        self.values = values

    def __iter__(self):
        return iter(self.values)

    def __ror__(self, other):
        return "ror"

    def __rand__(self, other):
        return "rand"

    def __rsub__(self, other):
        return "rsub"

    def __rxor__(self, other):
        return "rxor"


def non_set_comparisons(label, view):
    for other in ([], NonSet()):
        print(
            "direct-nonset",
            label,
            type(other).__name__,
            outcome(lambda: view.__eq__(other) is NotImplemented),
            outcome(lambda: view.__ne__(other) is NotImplemented),
            outcome(lambda: view.__lt__(other) is NotImplemented),
            outcome(lambda: view.__le__(other) is NotImplemented),
            outcome(lambda: view.__gt__(other) is NotImplemented),
            outcome(lambda: view.__ge__(other) is NotImplemented),
        )
        print(
            "native-nonset",
            label,
            type(other).__name__,
            view == other,
            view != other,
            outcome(lambda: view < other),
        )


def custom_iterable_set_dunders(label, view, values):
    other = ReflectedIterable(values)
    print(
        "direct-custom-set",
        label,
        outcome(lambda: sorted(view.__and__(other))),
        outcome(lambda: sorted(view.__rand__(other))),
        outcome(lambda: sorted(view.__or__(other))),
        outcome(lambda: sorted(view.__ror__(other))),
        outcome(lambda: sorted(view.__sub__(other))),
        outcome(lambda: sorted(view.__rsub__(other))),
        outcome(lambda: sorted(view.__xor__(other))),
        outcome(lambda: sorted(view.__rxor__(other))),
    )


def items_order_outcomes(label, left, right):
    print(
        "items-order",
        label,
        "direct",
        outcome(lambda: left.__lt__(right)),
        outcome(lambda: left.__le__(right)),
        outcome(lambda: left.__gt__(right)),
        outcome(lambda: left.__ge__(right)),
        "native",
        outcome(lambda: left < right),
        outcome(lambda: left <= right),
        outcome(lambda: left > right),
        outcome(lambda: left >= right),
    )


def items_equality_outcomes(label, left, right):
    print(
        "items-equality",
        label,
        outcome(lambda: left.__eq__(right)),
        outcome(lambda: left.__ne__(right)),
        outcome(lambda: left == right),
        outcome(lambda: left != right),
    )


data = {"a": 1, "b": 2}
keys = data.keys()
items = data.items()
values = data.values()
values_alias = values
fresh_values = data.values()
ordered_data = OrderedDict((("a", 1), ("b", 2)))
ordered_keys = ordered_data.keys()
ordered_items = ordered_data.items()
ordered_values = ordered_data.values()

print(
    "hidden constructors",
    tuple(
        hasattr(builtins, name)
        for name in (
            "dict_keys",
            "dict_items",
            "dict_values",
            "odict_keys",
            "odict_items",
            "odict_values",
        )
    ),
)

class_shape("keys", keys, data.keys())
class_shape("items", items, data.items())
class_shape("values", values, data.values())
class_shape("ordered-keys", ordered_keys, ordered_data.keys())
class_shape("ordered-items", ordered_items, ordered_data.items())
class_shape("ordered-values", ordered_values, ordered_data.values())
print(
    "ordered-type-distinct",
    type(ordered_keys) is type(keys),
    type(ordered_items) is type(items),
    type(ordered_values) is type(values),
)
print(
    "view-subclasses",
    tuple(cls.__name__ for cls in type(keys).__subclasses__()),
    tuple(cls.__name__ for cls in type(items).__subclasses__()),
    tuple(cls.__name__ for cls in type(values).__subclasses__()),
)
print(
    "ordered-view-equality",
    ordered_keys == keys,
    keys == ordered_keys,
    ordered_items == items,
    items == ordered_items,
    ordered_values == values,
    values == ordered_values,
)
unbound_view_descriptors(keys, items, values)
print(
    "unbound-base-ordered",
    outcome(lambda: list(type(keys).__iter__(ordered_keys))),
    outcome(lambda: type(items).__contains__(ordered_items, ("a", 1))),
    outcome(lambda: type(values).__len__(ordered_values)),
)
print(
    "unbound-ordered-owned",
    outcome(lambda: list(type(ordered_keys).__iter__(ordered_keys))),
    outcome(lambda: list(type(ordered_items).__reversed__(ordered_items))),
    outcome(lambda: list(type(ordered_values).__iter__(ordered_values))),
)
for bound_label, bound_view, bound_names in (
    ("keys", keys, ("__iter__", "__reversed__", "isdisjoint")),
    ("items", items, ("__iter__", "__reversed__", "isdisjoint")),
    ("values", values, ("__iter__", "__reversed__")),
    (
        "ordered-keys",
        ordered_keys,
        ("__iter__", "__reversed__", "isdisjoint"),
    ),
    (
        "ordered-items",
        ordered_items,
        ("__iter__", "__reversed__", "isdisjoint"),
    ),
    ("ordered-values", ordered_values, ("__iter__", "__reversed__")),
):
    bound_view_method_shape(bound_label, bound_view, bound_names)
    view_descriptor_keyword_errors(bound_label, bound_view)
    default_error_shape(bound_label, bound_view)
    if bound_label in ("ordered-keys", "ordered-items"):
        saved_isdisjoint_error_shape(bound_label, bound_view)
        compiler_cutoff_isdisjoint_error_shape(bound_label, bound_view)
descriptor_inventory(
    "keys",
    type(keys),
    (
        "__repr__",
        "__getattribute__",
        "__iter__",
        "__len__",
        "__contains__",
        "__eq__",
        "__reversed__",
        "isdisjoint",
    ),
)
descriptor_inventory(
    "items",
    type(items),
    ("__repr__", "__iter__", "__contains__", "__eq__", "__reversed__", "isdisjoint"),
)
descriptor_inventory(
    "values",
    type(values),
    ("__repr__", "__getattribute__", "__iter__", "__len__", "__reversed__"),
)
for ordered_label, ordered_cls in (
    ("ordered-keys", type(ordered_keys)),
    ("ordered-items", type(ordered_items)),
    ("ordered-values", type(ordered_values)),
):
    descriptor_inventory(ordered_label, ordered_cls, ("__iter__", "__reversed__"))
print(
    "unbound-cross-view-errors",
    outcome(lambda: type(keys).__iter__(items)),
    outcome(lambda: type(items).__contains__(keys, ("a", 1))),
    outcome(lambda: type(values).__len__(keys)),
)
for view_label, view_mapping, view in (
    ("keys", data, keys),
    ("items", data, items),
    ("values", data, values),
    ("ordered-keys", ordered_data, ordered_keys),
    ("ordered-items", ordered_data, ordered_items),
    ("ordered-values", ordered_data, ordered_values),
):
    mapping_descriptor_shape(view_label, view_mapping, view)
    view_getattribute_shape(view_label, view)
print(
    "view-getattribute-errors",
    outcome(lambda: type(keys).__getattribute__(1, "real")),
    outcome(lambda: type(keys).__getattribute__()),
    outcome(lambda: type(keys).__getattribute__(keys, "mapping", 1)),
    outcome(lambda: type(keys).__getattribute__(self=keys, name="mapping")),
)

values_as_key = {values: "ok"}
values_as_set_member = {values}
print(
    "values-identity-hash",
    hash(values) == hash(values_alias),
    values is values_alias,
    values is not fresh_values,
    hash_outcome(fresh_values),
    values_as_key[values_alias] == "ok",
    values_alias in values_as_set_member,
    fresh_values in values_as_set_member,
)

key_exact = {"a", "b"}
key_smaller = {"a"}
key_larger = {"a", "b", "c"}
direct_set_dunders("keys", keys, {"b", "c"}, key_exact, key_smaller, key_larger)
native_set_ops("keys", keys, key_exact, key_smaller, key_larger)
non_set_comparisons("keys", keys)
custom_iterable_set_dunders("keys", keys, ["b", "c"])

item_exact = {("a", 1), ("b", 2)}
item_smaller = {("a", 1)}
item_larger = {("a", 1), ("b", 2), ("c", 3)}
direct_set_dunders("items", items, {("b", 2), ("c", 3)}, item_exact, item_smaller, item_larger)
native_set_ops("items", items, item_exact, item_smaller, item_larger)
non_set_comparisons("items", items)
custom_iterable_set_dunders("items", items, [("b", 2), ("c", 3)])

unhashable_data = {"a": []}
unhashable_items = unhashable_data.items()
unhashable_items_alias = unhashable_items
unhashable_items_fresh = unhashable_data.items()
unhashable_items_equivalent = {"a": []}.items()
unhashable_items_different = {"a": [1]}.items()
print(
    "items-unhashable",
    outcome(lambda: unhashable_items.__eq__(unhashable_items_alias)),
    outcome(lambda: unhashable_items.__eq__(unhashable_items_fresh)),
    outcome(lambda: unhashable_items.__eq__(unhashable_items_equivalent)),
    outcome(lambda: unhashable_items.__eq__(unhashable_items_different)),
    outcome(lambda: unhashable_items.__ne__(unhashable_items_different)),
    outcome(lambda: unhashable_items == unhashable_items_fresh),
    outcome(lambda: unhashable_items == unhashable_items_equivalent),
    outcome(lambda: unhashable_items == unhashable_items_different),
)

list_order_data = {"a": [], "b": [1]}
list_order_items = list_order_data.items()
for label, other in (
    ("list-same-view", list_order_items),
    ("list-same-map", list_order_data.items()),
    ("list-equal", {"a": [], "b": [1]}.items()),
    ("list-subset", {"a": []}.items()),
    ("list-superset", {"a": [], "b": [1], "c": [2]}.items()),
    ("list-different", {"a": [], "b": [2]}.items()),
):
    items_order_outcomes(label, list_order_items, other)

dict_order_data = {"a": {"x": 1}, "b": {"y": 2}}
dict_order_items = dict_order_data.items()
for label, other in (
    ("dict-same-view", dict_order_items),
    ("dict-same-map", dict_order_data.items()),
    ("dict-equal", {"a": {"x": 1}, "b": {"y": 2}}.items()),
    ("dict-subset", {"a": {"x": 1}}.items()),
    (
        "dict-superset",
        {"a": {"x": 1}, "b": {"y": 2}, "c": {"z": 3}}.items(),
    ),
    ("dict-different", {"a": {"x": 1}, "b": {"y": 3}}.items()),
):
    items_order_outcomes(label, dict_order_items, other)

single_unhashable_items = {"a": []}.items()
items_equality_outcomes("mixed-empty-set", single_unhashable_items, set())
items_equality_outcomes("mixed-empty-keys", single_unhashable_items, {}.keys())
items_equality_outcomes("mixed-empty-items", single_unhashable_items, {}.items())
items_equality_outcomes(
    "mixed-same-size-set",
    single_unhashable_items,
    {("a", 0)},
)
items_equality_outcomes(
    "mixed-larger-set",
    single_unhashable_items,
    {("a", 0), ("b", 1)},
)
items_order_outcomes("mixed-empty-set", single_unhashable_items, set())
items_order_outcomes("mixed-empty-keys", single_unhashable_items, {}.keys())
items_order_outcomes("mixed-empty-items", single_unhashable_items, {}.items())
items_order_outcomes("mixed-same-size-set", single_unhashable_items, {("a", 0)})
items_order_outcomes(
    "mixed-larger-set",
    single_unhashable_items,
    {("a", 0), ("b", 1)},
)

cross_view_hash_log = []
CROSS_VIEW_COMPARISONS = (
    ("keys-direct-eq-items", lambda keys, items: keys.__eq__(items)),
    ("keys-native-eq-items", lambda keys, items: keys == items),
    ("items-direct-eq-keys", lambda keys, items: items.__eq__(keys)),
    ("items-native-eq-keys", lambda keys, items: items == keys),
    ("keys-direct-ne-items", lambda keys, items: keys.__ne__(items)),
    ("keys-native-ne-items", lambda keys, items: keys != items),
    ("items-direct-ne-keys", lambda keys, items: items.__ne__(keys)),
    ("items-native-ne-keys", lambda keys, items: items != keys),
)


def cross_view_probe(label, keys, items):
    for operation_label, operation in CROSS_VIEW_COMPARISONS:
        cross_view_hash_log.clear()
        result = outcome(lambda: operation(keys, items))
        print(
            "cross-view",
            label,
            operation_label,
            result,
            tuple(cross_view_hash_log),
        )


class CrossViewHashValue:
    def __hash__(self):
        cross_view_hash_log.append("hash")
        return 29


cross_view_unhashable_data = {"a": []}
cross_view_unhashable_keys = cross_view_unhashable_data.keys()
cross_view_unhashable_items = cross_view_unhashable_data.items()
cross_view_probe(
    "unhashable",
    cross_view_unhashable_keys,
    cross_view_unhashable_items,
)

cross_view_set = {("a", 0)}
print(
    "cross-view-set-slots",
    "set-direct-items",
    outcome(lambda: cross_view_set.__eq__(cross_view_unhashable_items)),
)
print(
    "cross-view-set-slots",
    "set-native-items",
    outcome(lambda: cross_view_set == cross_view_unhashable_items),
)
print(
    "cross-view-set-slots",
    "items-direct-set",
    outcome(lambda: cross_view_unhashable_items.__eq__(cross_view_set)),
)
print(
    "cross-view-set-slots",
    "items-native-set",
    outcome(lambda: cross_view_unhashable_items == cross_view_set),
)


cross_view_hashable_data = {"a": CrossViewHashValue()}
cross_view_hashable_keys = cross_view_hashable_data.keys()
cross_view_hashable_items = cross_view_hashable_data.items()
cross_view_probe(
    "hashable",
    cross_view_hashable_keys,
    cross_view_hashable_items,
)

item_compare_log = []


class ItemCompareValue:
    def __init__(self, name, result, error=None):
        self.name = name
        self.result = result
        self.error = error

    def __eq__(self, other):
        item_compare_log.append((self.name, other.name))
        if self.error is not None:
            raise RuntimeError(self.error)
        return self.result


def item_comparison_probe(label, fn):
    item_compare_log.clear()
    result = outcome(fn)
    print("items-value-eq", label, result, tuple(item_compare_log))


direction_lhs = ItemCompareValue("lhs", False)
direction_rhs = ItemCompareValue("rhs", True)
direction_lhs_items = {"key": direction_lhs}.items()
direction_rhs_items = {"key": direction_rhs}.items()
item_comparison_probe(
    "direct-eq-direction",
    lambda: direction_lhs_items.__eq__(direction_rhs_items),
)
item_comparison_probe(
    "native-eq-direction",
    lambda: direction_lhs_items == direction_rhs_items,
)
item_comparison_probe(
    "direct-le-direction",
    lambda: direction_lhs_items.__le__(direction_rhs_items),
)
item_comparison_probe(
    "native-le-direction",
    lambda: direction_lhs_items <= direction_rhs_items,
)
item_comparison_probe(
    "direct-ge-direction",
    lambda: direction_lhs_items.__ge__(direction_rhs_items),
)
item_comparison_probe(
    "native-ge-direction",
    lambda: direction_lhs_items >= direction_rhs_items,
)

reflected_lhs = ItemCompareValue("reflected-lhs", True)
reflected_rhs = ItemCompareValue("reflected-rhs", NotImplemented)
reflected_lhs_items = {"key": reflected_lhs}.items()
reflected_rhs_items = {"key": reflected_rhs}.items()
item_comparison_probe(
    "direct-eq-reflected",
    lambda: reflected_lhs_items.__eq__(reflected_rhs_items),
)
item_comparison_probe(
    "native-le-reflected",
    lambda: reflected_lhs_items <= reflected_rhs_items,
)

shared_value = ItemCompareValue("identity", False, "identity comparison called")
shared_lhs_items = {"key": shared_value}.items()
shared_rhs_items = {"key": shared_value}.items()
item_comparison_probe(
    "direct-eq-identity",
    lambda: shared_lhs_items.__eq__(shared_rhs_items),
)
item_comparison_probe(
    "native-le-identity",
    lambda: shared_lhs_items <= shared_rhs_items,
)

error_lhs = ItemCompareValue("error-lhs", True)
error_rhs = ItemCompareValue("error-rhs", True, "items value comparison")
error_lhs_items = {"key": error_lhs}.items()
error_rhs_items = {"key": error_rhs}.items()
item_comparison_probe(
    "direct-eq-error",
    lambda: error_lhs_items.__eq__(error_rhs_items),
)
item_comparison_probe(
    "native-le-error",
    lambda: error_lhs_items <= error_rhs_items,
)

item_key_log = []


class ItemCompareKey:
    def __init__(self, name, fail_hash=False):
        self.name = name
        self.fail_hash = fail_hash

    def __hash__(self):
        item_key_log.append(("hash", self.name))
        if self.fail_hash:
            raise RuntimeError("items key hash")
        return 17

    def __eq__(self, other):
        item_key_log.append(("eq", self.name, other.name))
        return True


def item_key_probe(label, fn):
    item_key_log.clear()
    result = outcome(fn)
    print("items-key", label, result, tuple(item_key_log))


direction_lhs_key = ItemCompareKey("lhs-key")
direction_rhs_key = ItemCompareKey("rhs-key")
direction_lhs_key_items = {direction_lhs_key: []}.items()
direction_rhs_key_items = {direction_rhs_key: []}.items()
item_key_probe(
    "direct-eq-direction",
    lambda: direction_lhs_key_items.__eq__(direction_rhs_key_items),
)
item_key_probe(
    "native-le-direction",
    lambda: direction_lhs_key_items <= direction_rhs_key_items,
)

same_map_key = ItemCompareKey("same-map-key")
same_key_map = {same_map_key: []}
same_key_items = same_key_map.items()
item_key_probe(
    "direct-eq-same-view",
    lambda: same_key_items.__eq__(same_key_items),
)
item_key_probe(
    "native-eq-same-view",
    lambda: same_key_items == same_key_items,
)
item_key_probe(
    "direct-eq-same-map",
    lambda: same_key_items.__eq__(same_key_map.items()),
)
item_key_probe(
    "native-le-same-map",
    lambda: same_key_items <= same_key_map.items(),
)

error_lhs_key = ItemCompareKey("error-lhs-key")
error_rhs_key = ItemCompareKey("error-rhs-key")
error_lhs_key_items = {error_lhs_key: []}.items()
error_rhs_key_items = {error_rhs_key: []}.items()
error_lhs_key.fail_hash = True
item_key_probe(
    "direct-eq-hash-error",
    lambda: error_lhs_key_items.__eq__(error_rhs_key_items),
)
item_key_probe(
    "native-le-hash-error",
    lambda: error_lhs_key_items <= error_rhs_key_items,
)


live_item_value_log = []


class LiveItemValueMutation:
    def __init__(self, target, mode):
        self.target = target
        self.mode = mode

    def __eq__(self, other):
        live_item_value_log.append(("mutate", self.mode, other))
        if self.mode == "future-value":
            self.target["b"] = "new"
        elif self.mode == "current-value":
            self.target["a"] = "changed"
        else:
            del self.target["b"]
            self.target["b"] = "new"
        return True


class ExpectedLiveItemValue:
    def __init__(self, expected):
        self.expected = expected

    def __eq__(self, other):
        live_item_value_log.append(("expect", self.expected, other))
        return other == self.expected


def live_item_value_probe(label, mode, operation):
    lhs = {"a": "first", "b": "old"}
    expected = "old" if mode == "current-value" else "new"
    rhs = {
        "a": LiveItemValueMutation(lhs, mode),
        "b": ExpectedLiveItemValue(expected),
    }
    live_item_value_log.clear()
    result = outcome(lambda: operation(lhs.items(), rhs.items()))
    print(
        "items-live-value",
        label,
        result,
        tuple(live_item_value_log),
        tuple(lhs.items()),
    )


live_item_value_probe(
    "direct-eq-future-value",
    "future-value",
    lambda left, right: left.__eq__(right),
)
live_item_value_probe(
    "native-eq-future-value",
    "future-value",
    lambda left, right: left == right,
)
live_item_value_probe(
    "direct-le-future-value",
    "future-value",
    lambda left, right: left.__le__(right),
)
live_item_value_probe(
    "native-le-future-value",
    "future-value",
    lambda left, right: left <= right,
)
live_item_value_probe(
    "direct-eq-current-value",
    "current-value",
    lambda left, right: left.__eq__(right),
)
live_item_value_probe(
    "native-le-future-reinsert",
    "future-reinsert",
    lambda left, right: left <= right,
)


class OrderedItemMutation:
    def __init__(self, target, mode):
        self.target = target
        self.mode = mode

    def __eq__(self, other):
        if self.mode == "add":
            self.target["added"] = "new"
        elif self.mode == "clear":
            self.target.clear()
        elif self.mode == "reinsert":
            del self.target["b"]
            self.target["b"] = "second"
        elif self.mode == "restore":
            self.target.move_to_end("a")
            self.target.move_to_end("a", last=False)
        else:
            self.target.move_to_end("a")
        return True


class OrderedItemExpected:
    def __eq__(self, other):
        return other == "second"


ORDERED_ITEM_COMPARISONS = (
    ("direct-eq", lambda left, right: left.__eq__(right)),
    ("native-eq", lambda left, right: left == right),
    ("direct-le", lambda left, right: left.__le__(right)),
    ("native-le", lambda left, right: left <= right),
)


def ordered_item_mutation_probe(mode, label, operation):
    pairs = (("a", "first"), ("b", "second"), ("c", "third"))
    lhs = OrderedDict(pairs if mode == "restore" else pairs[:2])
    rhs = {
        "a": OrderedItemMutation(lhs, mode),
        "b": OrderedItemExpected(),
    }
    if mode == "restore":
        rhs["c"] = "third"
    result = outcome(lambda: operation(lhs.items(), rhs.items()))
    print("ordered-items-mutation", mode, label, result, tuple(lhs.items()))


for ordered_mutation_mode in ("add", "clear", "reinsert", "move", "restore"):
    for ordered_operation_label, ordered_operation in ORDERED_ITEM_COMPARISONS:
        ordered_item_mutation_probe(
            ordered_mutation_mode,
            ordered_operation_label,
            ordered_operation,
        )


class MixedItemRehashKey:
    def __init__(self):
        self.calls = 0
        self.fail = False

    def __hash__(self):
        self.calls += 1
        if self.fail:
            raise RuntimeError("rehash")
        return 17


def mixed_item_rehash_probe(label, key, operation):
    key.calls = 0
    result = outcome(operation)
    print("items-mixed-rehash", label, result, key.calls)


mixed_rehash_key = MixedItemRehashKey()
mixed_rehash_items = {mixed_rehash_key: 1}.items()
mixed_rehash_equal = {(mixed_rehash_key, 1)}
mixed_rehash_larger = {(mixed_rehash_key, 1), ("extra", 2)}
mixed_rehash_key.fail = True
mixed_item_rehash_probe(
    "native-eq",
    mixed_rehash_key,
    lambda: mixed_rehash_items == mixed_rehash_equal,
)
mixed_item_rehash_probe(
    "native-ne",
    mixed_rehash_key,
    lambda: mixed_rehash_items != mixed_rehash_equal,
)
mixed_item_rehash_probe(
    "direct-eq",
    mixed_rehash_key,
    lambda: mixed_rehash_items.__eq__(mixed_rehash_equal),
)
mixed_item_rehash_probe(
    "native-lt",
    mixed_rehash_key,
    lambda: mixed_rehash_items < mixed_rehash_larger,
)
mixed_item_rehash_probe(
    "native-le",
    mixed_rehash_key,
    lambda: mixed_rehash_items <= mixed_rehash_larger,
)
mixed_item_rehash_probe(
    "reflected-gt",
    mixed_rehash_key,
    lambda: mixed_rehash_larger > mixed_rehash_items,
)
mixed_item_rehash_probe(
    "direct-lt",
    mixed_rehash_key,
    lambda: mixed_rehash_items.__lt__(mixed_rehash_larger),
)
mixed_item_rehash_probe(
    "native-and-empty",
    mixed_rehash_key,
    lambda: mixed_rehash_items & set(),
)


class LateItemHash:
    def __init__(self):
        self.calls = 0

    def __hash__(self):
        self.calls += 1
        raise RuntimeError("late item hash")


def lazy_item_hash_probe(label, value, operation):
    value.calls = 0
    result = outcome(operation)
    print("items-lazy-hash", label, result, value.calls)


late_item_hash = LateItemHash()
lazy_hash_items = {"a": 1, "b": late_item_hash}.items()
lazy_hash_equal_size = {("missing-a", 1), ("missing-b", 2)}
lazy_hash_larger = {("missing-a", 1), ("missing-b", 2), ("missing-c", 3)}
lazy_item_hash_probe(
    "direct-eq-first-miss",
    late_item_hash,
    lambda: lazy_hash_items.__eq__(lazy_hash_equal_size),
)
lazy_item_hash_probe(
    "native-eq-first-miss",
    late_item_hash,
    lambda: lazy_hash_items == lazy_hash_equal_size,
)
lazy_item_hash_probe(
    "direct-le-first-miss",
    late_item_hash,
    lambda: lazy_hash_items.__le__(lazy_hash_equal_size),
)
lazy_item_hash_probe(
    "native-le-first-miss",
    late_item_hash,
    lambda: lazy_hash_items <= lazy_hash_equal_size,
)
lazy_item_hash_probe(
    "direct-lt-first-miss",
    late_item_hash,
    lambda: lazy_hash_items.__lt__(lazy_hash_larger),
)
lazy_item_hash_probe(
    "native-lt-first-miss",
    late_item_hash,
    lambda: lazy_hash_items < lazy_hash_larger,
)


set_source_mutation_log = []


class MutatingSetSourceValue:
    def __init__(self, target):
        self.target = target

    def __eq__(self, other):
        set_source_mutation_log.append(other)
        self.target.add(("added", 2))
        return True


def set_source_mutation_probe(label, operation):
    source = {("key", 1)}
    target = {"key": MutatingSetSourceValue(source)}.items()
    set_source_mutation_log.clear()
    result = outcome(lambda: operation(source, target))
    print(
        "items-set-source-mutation",
        label,
        result,
        tuple(set_source_mutation_log),
        len(source),
    )


set_source_mutation_probe(
    "native-le",
    lambda source, target: source <= target,
)
set_source_mutation_probe(
    "direct-items-ge",
    lambda source, target: target.__ge__(source),
)


class MutatingKeysSourceValue:
    def __init__(self, target):
        self.target = target

    def __eq__(self, other):
        self.target[("added", 2)] = None
        return True


def keys_source_mutation_probe(label, operation):
    mapping = {("key", 1): None}
    source = mapping.keys()
    target = {"key": MutatingKeysSourceValue(mapping)}.items()
    result = outcome(lambda: operation(source, target))
    print("items-keys-source-mutation", label, result, len(mapping))


keys_source_mutation_probe(
    "native-le",
    lambda source, target: source <= target,
)
keys_source_mutation_probe(
    "direct-items-ge",
    lambda source, target: target.__ge__(source),
)


class MutatingItemValue:
    def __init__(self, target, replace_key=False):
        self.target = target
        self.replace_key = replace_key

    def __eq__(self, other):
        if self.replace_key:
            del self.target["key"]
            self.target["replacement"] = {}
        else:
            self.target["added"] = {}
        return True


def items_mutation_probe(label, target_side, operation):
    lhs = {"key": []}
    rhs = {}
    target = lhs if target_side == "lhs" else rhs
    rhs["key"] = MutatingItemValue(target)
    result = outcome(lambda: operation(lhs.items(), rhs.items()))
    print("items-mutation", label, result, len(lhs), len(rhs))


items_mutation_probe("direct-eq-grow-lhs", "lhs", lambda left, right: left.__eq__(right))
items_mutation_probe("native-le-grow-lhs", "lhs", lambda left, right: left <= right)
items_mutation_probe("direct-eq-grow-rhs", "rhs", lambda left, right: left.__eq__(right))
items_mutation_probe("native-le-grow-rhs", "rhs", lambda left, right: left <= right)

same_size_mutation_lhs = {"key": []}
same_size_mutation_rhs = {
    "key": MutatingItemValue(same_size_mutation_lhs, replace_key=True)
}
print(
    "items-mutation",
    "direct-eq-replace-lhs-key",
    outcome(
        lambda: same_size_mutation_lhs.items().__eq__(same_size_mutation_rhs.items())
    ),
    len(same_size_mutation_lhs),
    len(same_size_mutation_rhs),
)

print(
    "direct-values",
    outcome(lambda: values.__len__()),
    outcome(lambda: list(values.__iter__())),
    outcome(lambda: values.__eq__(values)),
    outcome(lambda: values.__eq__(data.values()) is NotImplemented),
    outcome(lambda: values.__ne__(values)),
    outcome(lambda: values.__le__(values) is NotImplemented),
    outcome(lambda: values.__lt__(values) is NotImplemented),
    outcome(lambda: values.__ge__(values) is NotImplemented),
    outcome(lambda: values.__gt__(values) is NotImplemented),
)
print(
    "native-values",
    1 in values,
    len(values),
    list(reversed(values)),
    values == values,
    values == data.values(),
)


class ValuesPeer:
    def __eq__(self, other):
        return type(other).__name__ == "dict_values"

    def __ne__(self, other):
        return False


peer = ValuesPeer()
print(
    "values-reflected",
    values == peer,
    peer == values,
    values != peer,
    peer != values,
    values.__eq__(peer) is NotImplemented,
    values.__ne__(peer) is NotImplemented,
)

data["c"] = 3
data["a"] = 10
del data["b"]
print("live", list(keys), list(items), list(values))

for label, view in (
    ("keys", {"a": 1}.keys()),
    ("items", {"a": 1}.items()),
    ("values", {"a": 1}.values()),
    ("ordered-keys", OrderedDict((("a", 1),)).keys()),
    ("ordered-items", OrderedDict((("a", 1),)).items()),
    ("ordered-values", OrderedDict((("a", 1),)).values()),
):
    cls = type(view)
    print(
        "immutable-delete",
        label,
        immutable_delete_outcome(cls, "__len__"),
        immutable_delete_outcome(cls, "missing"),
        outcome(lambda: len(view)),
        hasattr(cls, "__len__"),
    )
