# CallMemo purity must account for every expression and mutation encoded outside
# a statement's obvious RHS/body fields.  Repeated equal integer calls are used
# deliberately: a false "memo-pure" classification would skip the second
# evaluation and expose stale state.


class Counter:
    def __init__(self):
        self.value = 0


attr_counter = Counter()


def mutate_attr(amount):
    attr_counter.value += amount
    return amount


print(mutate_attr(2), attr_counter.value)
print(mutate_attr(2), attr_counter.value)


index_counter = [0]


def mutate_index(amount):
    index_counter[0] += amount
    return amount


print(mutate_index(3), index_counter)
print(mutate_index(3), index_counter)


guard_enabled = False


def guarded_match(value):
    match value:
        case 1 if guard_enabled:
            return 10
        case _:
            return 20


print(guarded_match(1))
guard_enabled = True
print(guarded_match(1))


class PatternOwner:
    expected = 1


def value_pattern(value):
    match value:
        case PatternOwner.expected:
            return 30
        case _:
            return 40


print(value_pattern(1))
PatternOwner.expected = 2
print(value_pattern(1))


handled_kind = UnboundLocalError


def dynamic_handler(value):
    if False:
        missing = 0
    try:
        return missing
    except handled_kind:
        return value + 50


print(dynamic_handler(1))
handled_kind = TypeError
try:
    print(dynamic_handler(1))
except UnboundLocalError:
    print("unbound")


# AugAssign target expressions are reads for closure analysis.  These names are
# intentionally not mentioned anywhere else in the nested functions.
def make_attr_mutator(target):
    def mutate(amount):
        target.value += amount
        return amount

    return mutate


closure_attr = Counter()
closure_attr_mutate = make_attr_mutator(closure_attr)
print(closure_attr_mutate(4), closure_attr.value)


def make_index_mutator(target, position):
    def mutate(amount):
        target[position] += amount
        return amount

    return mutate


closure_index = [5]
closure_index_mutate = make_index_mutator(closure_index, 0)
print(closure_index_mutate(6), closure_index)


def make_slice_mutator(target, lower, upper, stride):
    def mutate(extra):
        target[lower:upper:stride] += extra
        return 1

    return mutate


closure_slice = [0, 1, 2, 3, 4]
closure_slice_mutate = make_slice_mutator(closure_slice, 0, 5, 2)
print(closure_slice_mutate([]), closure_slice)
