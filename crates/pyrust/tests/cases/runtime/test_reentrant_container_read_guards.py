# Safe RefCell read guards must be released before user callbacks. These cases
# also pin CPython's live-vs-snapshot behaviour when that callback mutates the
# container currently being compared or rendered.


class ListEq:
    def __eq__(self, other):
        left_list.append(2)
        return True


left_list = [ListEq()]
right_list = [ListEq()]
print("list-eq", left_list == right_list, len(left_list))


class DictKey:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        left_dict["added"] = 2
        return True


left_dict = {DictKey(): 1}
right_dict = {DictKey(): 1}
print("dict-eq", left_dict == right_dict, len(left_dict))


class SetItem:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        left_set.add(2)
        return True


left_set = {SetItem()}
right_set = {SetItem()}
print("set-eq", left_set == right_set, len(left_set))


class ListRepr:
    def __repr__(self):
        repr_list.append(2)
        return "R"


repr_list = [ListRepr()]
print("list-repr", repr(repr_list), len(repr_list))


class DictRepr:
    def __repr__(self):
        repr_dict["b"] = 2
        return "R"


repr_dict = {"a": DictRepr()}
print("dict-repr", repr(repr_dict), len(repr_dict))


class SetRepr:
    def __hash__(self):
        return 1

    def __repr__(self):
        repr_set.add(2)
        return "S"


repr_set = {SetRepr()}
print("set-repr", repr(repr_set), len(repr_set))
