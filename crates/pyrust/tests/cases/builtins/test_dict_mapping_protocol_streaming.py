# Mapping-protocol updates have two distinct phases in CPython:
#
# 1. materialise the iterable returned by keys();
# 2. resolve each key with __getitem__ and commit that pair immediately.
#
# A keys-iterator error therefore leaves the destination untouched, while a
# later __getitem__ error preserves the successfully resolved prefix.


class LookupBoom(Exception):
    pass


lookup_events = []


class LookupMap:
    def keys(self):
        lookup_events.append("keys-call")

        def key_stream():
            for key in ("first", "second", "unused"):
                lookup_events.append("key-" + key)
                yield key

        return key_stream()

    def __getitem__(self, key):
        lookup_events.append("get-" + key)
        if key == "second":
            raise LookupBoom("lookup failed")
        return key.upper()


updated = {"before": 0}
try:
    updated.update(LookupMap(), after=99)
except LookupBoom as exc:
    print("lookup error:", str(exc))
print("lookup partial:", list(updated.items()))
print("lookup events:", lookup_events)


class KeysBoom(Exception):
    pass


keys_events = []


class FailingLazyKeysMap:
    def keys(self):
        keys_events.append("keys-call")

        # An unbounded-shaped lazy source whose finite test prefix ends in an
        # error.  No __getitem__ call may occur before this iterator finishes.
        def key_stream():
            index = 0
            while True:
                keys_events.append("key-" + str(index))
                if index == 3:
                    raise KeysBoom("keys failed")
                yield "k" + str(index)
                index += 1

        return key_stream()

    def __getitem__(self, key):
        keys_events.append("get-" + key)
        return key


untouched = {"kept": 1}
try:
    untouched.update(FailingLazyKeysMap())
except KeysBoom as exc:
    print("keys error:", str(exc))
print("keys untouched:", list(untouched.items()))
print("keys events:", keys_events)


# A dict-subclass source may share the exact native backing with the receiver.
# The mapping visitor must snapshot that backing before invoking mutating
# callbacks.
class SelfMapping(dict):
    pass


same = SelfMapping()
same["a"] = 1
same["b"] = 2
same.update(same)
print("self update:", list(dict.items(same)))


# dict.__init__ updates an existing backing in place.  It must preserve both
# pre-existing entries and the prefix completed before __getitem__ fails.
class Reinitialised(dict):
    def __init__(self, source=None):
        if source is None:
            super().__init__()
        else:
            super().__init__(source)


reinitialised = Reinitialised()
reinitialised["before"] = 0
try:
    reinitialised.__init__(LookupMap())
except LookupBoom:
    pass
print("init partial:", list(dict.items(reinitialised)))

# With no source, dict.__init__ is a no-op rather than a clear operation.
reinitialised.__init__()
print("init no-op:", list(dict.items(reinitialised)))
