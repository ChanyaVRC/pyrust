# Metaclass __prepare__ protocol (issue #2128).
#
# CPython calls type(metaclass).__prepare__(name, bases, **kwds) before the
# class body runs, and uses the returned mapping as the body namespace.

# --- type.__prepare__ exists and returns a fresh dict ---
print(hasattr(type, '__prepare__'))
print(type.__prepare__('N', ()))
print(type.__prepare__('N', ()) == {})

# --- A custom __prepare__ pre-populating the namespace is visible on the class ---
class Meta(type):
    @classmethod
    def __prepare__(mcs, name, bases, **kw):
        d = {}
        d['injected'] = 'from_prepare'
        return d

class K(metaclass=Meta):
    x = 1

print(K.injected)
print(K.x)

# --- A recording mapping observes the body assignments in CPython's order ---
class Recorder(dict):
    def __init__(self):
        super().__init__()
        self.order = []
    def __setitem__(self, k, v):
        self.order.append(k)
        super().__setitem__(k, v)

class RecMeta(type):
    @classmethod
    def __prepare__(mcs, name, bases, **kw):
        return Recorder()
    def __new__(mcs, name, bases, ns):
        print("recorded:", ns.order)
        return super().__new__(mcs, name, bases, dict(ns))

class C(metaclass=RecMeta):
    a = 1
    b = 2
    def f(self):
        return 0

# --- super().__prepare__ resolves to type.__prepare__ ---
class SuperMeta(type):
    @classmethod
    def __prepare__(mcs, name, bases, **kw):
        ns = super().__prepare__(name, bases, **kw)
        ns['seeded'] = True
        return ns

class S(metaclass=SuperMeta):
    pass

print(S.seeded)

# --- A plain class (metaclass=type) is unaffected ---
class Plain:
    v = 10
print(Plain.v)
print(hasattr(Plain, 'injected'))

print("prepare OK")
