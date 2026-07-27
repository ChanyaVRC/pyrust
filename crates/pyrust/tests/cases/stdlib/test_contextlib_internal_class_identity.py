import contextlib
import sys


events = []


@contextlib.contextmanager
def managed(label):
    events.append("enter-" + label)
    try:
        yield label
    finally:
        events.append("exit-" + label)


@contextlib.contextmanager
def another_manager():
    yield None


old_one = managed("one")
old_two = managed("two")
old_type = type(old_one)
print(
    "same generation:",
    type(managed) is type(another_manager),
    type(old_one) is type(old_two),
    old_type is contextlib._GeneratorContextManager,
    old_type.__module__,
    contextlib.ExitStack.__module__,
    contextlib.suppress.__module__,
)

old_stack = contextlib.ExitStack()


def old_callback():
    events.append("old-callback")


old_stack.callback(old_callback)
with old_one as value:
    events.append("body-" + value)
print("old before reload:", events)

# Deleting the cache entry and importing again creates a new module generation.
# Its internal factory must consistently use that generation's class, while an
# instance made by the old generation retains its original type and behaviour.
del sys.modules["contextlib"]
import contextlib as reloaded_contextlib


@reloaded_contextlib.contextmanager
def reloaded_managed(label):
    events.append("enter-" + label)
    try:
        yield label
    finally:
        events.append("exit-" + label)


new_one = reloaded_managed("three")
new_two = reloaded_managed("four")
new_type = type(new_one)
print(
    "reimport generation:",
    type(new_one) is type(new_two),
    new_type is reloaded_contextlib._GeneratorContextManager,
    new_type is old_type,
    new_type.__module__,
    reloaded_contextlib.ExitStack.__module__,
)

with old_two as value:
    events.append("body-" + value)
with new_one as value:
    events.append("body-" + value)
old_stack.close()
print(
    "old instance alive:",
    type(old_two) is old_type,
    type(old_two) is new_type,
    events,
)
