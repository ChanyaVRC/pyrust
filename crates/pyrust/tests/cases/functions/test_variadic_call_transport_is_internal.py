import builtins


helper_calls = []


def __vcall__(*args, **kwargs):
    helper_calls.append((args, kwargs))
    return "hijacked"


def target(*args, **kwargs):
    return (args, sorted(kwargs.items()))


print(hasattr(builtins, "__vcall__"))
print(target(*[1], *[2], answer=42))
print(target(0, *[1], left=2, **{"right": 3}))
print(helper_calls)
