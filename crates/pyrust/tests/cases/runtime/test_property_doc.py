# Parity fixture for issue #1961: property.__doc__ exposes the getter
# docstring (or the explicit doc= argument).

class P:
    @property
    def v(self):
        "doc here"
        return 1

    @property
    def nodoc(self):
        return 2

    explicit = property(lambda s: 3, doc="explicit doc")

# Getter docstring is exposed via __doc__.
print(P.v.__doc__)            # doc here
# A property with no getter docstring reports None.
print(P.nodoc.__doc__)        # None
# An explicit doc= argument wins.
print(P.explicit.__doc__)     # explicit doc

# doc=None behaves like no explicit doc (falls back to getter docstring).
def getter_with_doc(self):
    "fallback doc"
    return 4

p_none = property(getter_with_doc, doc=None)
print(p_none.__doc__)         # fallback doc

# The .setter chain preserves the getter's docstring.
class Q:
    @property
    def w(self):
        "w doc"
        return 5

    @w.setter
    def w(self, value):
        pass

print(Q.w.__doc__)            # w doc

# The type is still 'property'.
print(type(P.v).__name__)     # property
