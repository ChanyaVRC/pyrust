"""Builtin slot names do not replace descriptor-owner provenance."""


def show(label, operation):
    try:
        print(label, "result", operation())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


class ListWithDictGetitem(list):
    __getitem__ = dict.__getitem__


class ChildListWithDictGetitem(ListWithDictGetitem):
    pass


show("index direct", lambda: ListWithDictGetitem([10, 20])[0])
show("index inherited", lambda: ChildListWithDictGetitem([10, 20])[0])
show("slice direct", lambda: ListWithDictGetitem([10, 20])[:1])


setitem_events = []


class RecordingDict(dict):
    def __setitem__(self, key, value):
        setitem_events.append((key, value))


recording = RecordingDict()
recording[1] = 10
recording["two"] = 20
print("setitem primitive keys", setitem_events, len(recording))


class DictWithListSetitem(dict):
    __setitem__ = list.__setitem__


def cross_owner_setitem():
    value = DictWithListSetitem()
    value[0] = 1
    return value


show("setitem cross owner", cross_owner_setitem)


class ListWithSetIor(list):
    __ior__ = set.__ior__


def cross_owner_list_ior():
    value = ListWithSetIor([1])
    value |= {2}
    return value


show("iop list/set", cross_owner_list_ior)


class DictWithListIadd(dict):
    __ior__ = list.__iadd__


def cross_owner_dict_ior():
    value = DictWithListIadd(a=1)
    value |= [("b", 2)]
    return value


show("iop dict/list", cross_owner_dict_ior)
