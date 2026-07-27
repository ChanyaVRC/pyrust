# CPython 3.12 parity for warnings category identity and subclass matching.

import warnings


class ParentWarning(UserWarning):
    pass


class ChildWarning(ParentWarning):
    pass


# A filter for a base warning category matches its subclasses, and action
# "error" raises the requested concrete warning class.
warnings.resetwarnings()
warnings.simplefilter("ignore")
warnings.filterwarnings("error", category=ParentWarning)
try:
    warnings.warn("child", ChildWarning)
except ChildWarning as caught:
    print("subclass", type(caught) is ChildWarning, str(caught))
else:
    print("subclass FAIL")


# Equal visible names do not make distinct classes interchangeable.
class SameName(UserWarning):
    pass


FirstSameName = SameName


class SameName(UserWarning):
    pass


SecondSameName = SameName
warnings.resetwarnings()
warnings.simplefilter("ignore")
warnings.filterwarnings("error", category=FirstSameName)
try:
    warnings.warn("second", SecondSameName)
except FirstSameName:
    print("same-name FAIL")
else:
    print("same-name distinct")
try:
    warnings.warn("first", FirstSameName)
except FirstSameName as caught:
    print("same-name first", type(caught) is FirstSameName)


# Renaming a class after installing the filter preserves its identity.
class RenamedWarning(UserWarning):
    pass


warnings.resetwarnings()
warnings.simplefilter("ignore")
warnings.filterwarnings("error", category=RenamedWarning)
RenamedWarning.__name__ = "VisibleWarning"
try:
    warnings.warn("renamed", RenamedWarning)
except RenamedWarning as caught:
    print("renamed", type(caught) is RenamedWarning, type(caught).__name__)
else:
    print("renamed FAIL")


# A Warning instance determines the effective category.  An explicit category
# is ignored, action "error" re-raises that exact object, and recording retains
# both the original instance and its concrete class.
warnings.resetwarnings()
warnings.simplefilter("ignore")
warnings.filterwarnings("error", category=ParentWarning)
original = ChildWarning("original")
try:
    warnings.warn(original, 1)
except ChildWarning as caught:
    print("instance-error", caught is original, type(caught) is ChildWarning)
else:
    print("instance-error FAIL")


class InitializedWarning(ParentWarning):
    def __init__(self, payload):
        self.payload = payload
        super().__init__("initialized:" + payload)


try:
    warnings.warn("payload", InitializedWarning)
except InitializedWarning as caught:
    print("constructed-error", caught.payload, str(caught))

with warnings.catch_warnings(record=True) as recorded:
    warnings.simplefilter("always")
    recorded_original = ChildWarning("recorded")
    warnings.warn(recorded_original, RuntimeWarning)
    warnings.warn("constructed", ChildWarning)

print(
    "record-instance",
    recorded[0].category is ChildWarning,
    recorded[0].message is recorded_original,
)
print(
    "record-constructed",
    recorded[1].category is ChildWarning,
    type(recorded[1].message) is ChildWarning,
    str(recorded[1].message),
)


# catch_warnings snapshots retain the filter's class object, even if the class
# is renamed before the saved filters are restored.
class SavedWarning(UserWarning):
    pass


warnings.resetwarnings()
warnings.simplefilter("ignore")
warnings.filterwarnings("error", category=SavedWarning)
with warnings.catch_warnings():
    SavedWarning.__name__ = "RestoredWarning"
try:
    warnings.warn("restored", SavedWarning)
except SavedWarning as caught:
    print("restored-filter", type(caught) is SavedWarning, type(caught).__name__)
else:
    print("restored-filter FAIL")


# filterwarnings validates a Warning subclass immediately.
for label, category in [
    ("none", None),
    ("value", 1),
    ("object", object),
    ("exception", Exception),
    ("warning", Warning),
    ("custom", ChildWarning),
]:
    warnings.resetwarnings()
    try:
        warnings.filterwarnings("ignore", category=category)
    except BaseException as error:
        print("filter-category", label, type(error).__name__)
    else:
        print("filter-category", label, "ok")


# simplefilter deliberately performs no category validation at registration.
# Classes participate in ordinary issubclass matching; a non-class fails only
# when a warning reaches that filter.
warnings.resetwarnings()
warnings.simplefilter("ignore", object)
warnings.warn("object-superclass", ChildWarning)
print("simple-object ok")

warnings.resetwarnings()
warnings.simplefilter("ignore", Exception)
warnings.warn("exception-superclass", ChildWarning)
print("simple-exception ok")

warnings.resetwarnings()
warnings.simplefilter("ignore", 1)
print("simple-value registered")
try:
    warnings.warn("deferred-validation", ChildWarning)
except TypeError:
    print("simple-value TypeError")
