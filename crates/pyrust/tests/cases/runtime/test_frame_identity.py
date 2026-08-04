# Issue #3025: a live activation owns one weak-cached frame object. Repeated
# lookups reuse that object and its function-locals snapshot, while recursive
# and later activations remain distinct. A generator keeps that identity across
# resumes while rebinding its live f_back to the caller driving each resume.
import sys


module_frame = sys._getframe()
print("module frame identity:", module_frame is sys._getframe())
print(
    "module locals preserved:",
    module_frame.f_locals is module_frame.f_globals,
    module_frame.f_locals is globals(),
)


def identity_and_locals():
    first = sys._getframe()
    namespace = first.f_locals
    namespace["ghost"] = 9
    x = 1
    second = sys._getframe()
    print("function frame identity:", first is second)
    print("function locals identity:", namespace is second.f_locals)
    print("function locals refresh:", namespace.get("x"), namespace.get("ghost"))
    del x
    third = sys._getframe()
    third_namespace = third.f_locals
    print(
        "function locals delete:",
        third_namespace is namespace,
        "x" in namespace,
        namespace.get("ghost"),
    )
    return first


old_activation = identity_and_locals()
new_activation = identity_and_locals()
print("distinct activations:", old_activation is not new_activation)


def locals_without_retaining_frame():
    namespace = sys._getframe().f_locals
    namespace["ghost"] = 9
    x = 1
    refreshed = sys._getframe().f_locals
    same_mapping = namespace is refreshed
    del x
    final = sys._getframe().f_locals
    return same_mapping, final is namespace, "x" in namespace, namespace.get("ghost")


print("locals-only caching:", locals_without_retaining_frame())


def locals_refresh_on_getter():
    x = 1
    frame = sys._getframe()
    namespace = frame.f_locals
    x = 2
    sys._getframe()
    before_getter = namespace["x"]
    after_getter = frame.f_locals["x"]
    return before_getter, after_getter


print("function locals getter refresh:", locals_refresh_on_getter())


iterated_locals = None
locals_key_iterator = None


def frame_locals_iterator_refresh():
    global iterated_locals, locals_key_iterator
    x = "old"
    iterated_locals = sys._getframe().f_locals
    locals_key_iterator = iter(iterated_locals)
    del x
    y = "new"
    sys._getframe().f_locals


frame_locals_iterator_refresh()
print(
    "function locals iterator refresh:",
    next(locals_key_iterator),
    list(iterated_locals),
)


def unbound_freevar_collision():
    x = 1

    def inner():
        if False:
            print(x)
        namespace = sys._getframe().f_locals
        namespace["x"] = 99
        namespace["ghost"] = 9
        refreshed = sys._getframe().f_locals
        return "x" in refreshed, refreshed.get("ghost")

    del x
    return inner()


print("unbound freevar refresh:", unbound_freevar_collision())


def unbound_comprehension_freevar_collision():
    def probe(frame):
        namespace = frame.f_locals
        namespace["x"] = 99
        namespace["ghost"] = 9
        refreshed = frame.f_locals
        return "x" in refreshed, refreshed.get("ghost")

    x = 1
    genexpr = (probe(sys._getframe()) if True else x for _ in (0,))
    del x
    return (
        next(genexpr),
        [probe(sys._getframe()) if True else x for _ in (0,)][0],
        next(iter({probe(sys._getframe()) if True else x for _ in (0,)})),
        {0: probe(sys._getframe()) if True else x for _ in (0,)}[0],
    )


print(
    "unbound comprehension freevar refresh:",
    unbound_comprehension_freevar_collision(),
)


module_candidate = "global"


def global_candidate_mapping_key():
    seen = module_candidate
    namespace = sys._getframe().f_locals
    namespace["module_candidate"] = "mapping-only"
    refreshed = sys._getframe().f_locals
    return namespace is refreshed, refreshed.get("module_candidate"), seen


print("global candidate preserved:", global_candidate_mapping_key())


def child(expected):
    direct = sys._getframe(1)
    through_back = sys._getframe().f_back
    again = sys._getframe(1)
    return direct is expected, through_back is expected, direct is again, direct is through_back


def parent():
    own = sys._getframe()
    return child(own)


print("caller identity:", parent())


def outer_chain_refresh():
    x = 1

    def inner():
        nonlocal x
        own = sys._getframe()
        before = own.f_back.f_locals["x"]
        x = 2
        again = sys._getframe()
        after = again.f_back.f_locals["x"]
        return own is again, before, after

    return inner()


print("caller locals refresh:", outer_chain_refresh())


def recursive(depth, frames):
    frames.append(sys._getframe())
    if depth:
        recursive(depth - 1, frames)


recursive_frames = []
recursive(3, recursive_frames)
print(
    "recursive activations:",
    len({id(frame) for frame in recursive_frames}) == len(recursive_frames),
)


def line_probe():
    frame = sys._getframe()
    first_line = frame.f_lineno
    marker = None
    same_frame = sys._getframe()
    second_line = same_frame.f_lineno
    return frame is same_frame, first_line < second_line, marker


print("line refresh:", line_probe()[:2])


def identity_generator():
    first = sys._getframe()
    yield first
    yield sys._getframe()


generator = identity_generator()
first_resume = next(generator)
second_resume = next(generator)
print("generator activation:", first_resume is second_resume)


def generator_object_frame_identity():
    frame = sys._getframe()
    namespace = frame.f_locals
    yield frame, namespace is frame.f_locals, namespace.get("mapping_only")


generator_object = generator_object_frame_identity()
prestart_frame = generator_object.gi_frame
prestart_locals = prestart_frame.f_locals
prestart_locals["mapping_only"] = 9
print("generator object frame before:", prestart_frame is generator_object.gi_frame)
active_frame, same_locals, mapping_only = next(generator_object)
suspended_frame = generator_object.gi_frame
print(
    "generator object frame shared:",
    prestart_frame is active_frame,
    active_frame is suspended_frame,
    suspended_frame is generator_object.gi_frame,
)
print("generator object frame active locals:", same_locals, mapping_only)
list(generator_object)
print("generator object frame exhausted:", generator_object.gi_frame is None)


def generator_object_locals_only():
    yield None


locals_only_generator = generator_object_locals_only()
retained_generator_locals = locals_only_generator.gi_frame.f_locals
locals_only_before = retained_generator_locals is locals_only_generator.gi_frame.f_locals
next(locals_only_generator)
locals_only_suspended = retained_generator_locals is locals_only_generator.gi_frame.f_locals
print("generator object locals-only caching:", locals_only_before, locals_only_suspended)
list(locals_only_generator)


generator_prestart_locals = None


def generator_object_locals_timing():
    x = 1
    frame = sys._getframe()
    before_getter = generator_prestart_locals.get("x")
    after_getter = frame.f_locals.get("x")
    yield before_getter, after_getter, frame.f_locals is generator_prestart_locals


locals_timing_generator = generator_object_locals_timing()
locals_timing_frame = locals_timing_generator.gi_frame
generator_prestart_locals = locals_timing_frame.f_locals
print("generator object locals getter refresh:", next(locals_timing_generator))
list(locals_timing_generator)


retained_direct_generator_frame = None
retained_direct_generator_locals = None


def make_direct_locals_generator():
    free_value = 1

    def body():
        local_value = 1
        if False:
            print(free_value)
        del local_value
        namespace = retained_direct_generator_frame.f_locals
        yield (
            namespace is retained_direct_generator_locals,
            "local_value" in namespace,
            "free_value" in namespace,
            namespace.get("mapping_only"),
        )

    generator = body()
    del free_value
    return generator


direct_locals_generator = make_direct_locals_generator()
retained_direct_generator_frame = direct_locals_generator.gi_frame
retained_direct_generator_locals = retained_direct_generator_frame.f_locals
retained_direct_generator_locals["local_value"] = 99
retained_direct_generator_locals["free_value"] = 99
retained_direct_generator_locals["mapping_only"] = 9
print("generator object direct locals refresh:", next(direct_locals_generator))
list(direct_locals_generator)


def changing_back_generator():
    frame = sys._getframe()
    yield frame, frame.f_back
    yield frame, frame.f_back


def resume_from_a(generator):
    caller = sys._getframe()
    frame, active_back = next(generator)
    return frame, caller is active_back, frame.f_back is None


def resume_from_b(generator):
    caller = sys._getframe()
    frame, active_back = next(generator)
    return frame, caller is active_back, frame.f_back is None


changing_back = changing_back_generator()
back_frame_a, active_back_a, cleared_back_a = resume_from_a(changing_back)
back_frame_b, active_back_b, cleared_back_b = resume_from_b(changing_back)
print(
    "generator caller refresh:",
    back_frame_a is back_frame_b,
    active_back_a,
    cleared_back_a,
    active_back_b,
    cleared_back_b,
)


nested_outer_frame = None


def nested_back_inner():
    inner_frame = sys._getframe()
    yield inner_frame, inner_frame.f_back, inner_frame.f_back.f_back


def nested_back_outer():
    global nested_outer_frame
    nested_outer_frame = sys._getframe()
    yield from nested_back_inner()


def nested_back_drive():
    caller = sys._getframe()
    generator = nested_back_outer()
    inner_frame, inner_back, outer_back = next(generator)
    return (
        inner_back is nested_outer_frame,
        outer_back is caller,
        inner_frame.f_back is None,
        nested_outer_frame.f_back is None,
    )


print("nested generator caller chain:", nested_back_drive())


def class_caller():
    return sys._getframe(1)


class ClassFrame:
    first = sys._getframe()
    namespace = first.f_locals
    second = sys._getframe()
    via_depth = class_caller()
    same_frame = first is second is via_depth
    same_locals = namespace is second.f_locals is via_depth.f_locals is locals()
    namespace["mapped"] = "live"
    mapped_read = mapped


print("class frame identity:", ClassFrame.same_frame)
print("class locals preserved:", ClassFrame.same_locals, ClassFrame.mapped_read)
