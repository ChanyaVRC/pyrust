# Parity fixture for issue #1253:
# staticmethod and classmethod must preserve their wrapper type.

class Foo:
    @staticmethod
    def sm(): return 42

    @classmethod
    def cm(cls): return cls

# ─── type names ────────────────────────────────────────────────────────────────
print(type(Foo.__dict__['sm']).__name__)   # staticmethod
print(type(Foo.__dict__['cm']).__name__)   # classmethod
print(type(staticmethod(lambda: 1)).__name__)          # staticmethod
print(type(classmethod(lambda cls: cls)).__name__)     # classmethod

# ─── isinstance ────────────────────────────────────────────────────────────────
print(isinstance(Foo.__dict__['sm'], staticmethod))   # True
print(isinstance(Foo.__dict__['cm'], classmethod))    # True
print(isinstance(staticmethod(lambda: 1), staticmethod))    # True
print(isinstance(classmethod(lambda cls: cls), classmethod)) # True
# negative: classmethod is NOT a staticmethod and vice versa
print(isinstance(Foo.__dict__['sm'], classmethod))    # False
print(isinstance(Foo.__dict__['cm'], staticmethod))   # False

# ─── __func__ attribute ────────────────────────────────────────────────────────
sm = Foo.__dict__['sm']
cm = Foo.__dict__['cm']
print(sm.__func__())          # 42
print(cm.__func__(Foo) is Foo)  # True

# Direct construction
sm2 = staticmethod(lambda: 99)
cm2 = classmethod(lambda cls: cls.__name__)
print(sm2.__func__())          # 99
print(cm2.__func__(Foo))       # Foo

# ─── descriptor protocol (calling) still works ─────────────────────────────────
print(Foo.sm())                # 42
print(Foo.cm() is Foo)         # True
obj = Foo()
print(obj.sm())                # 42
print(obj.cm() is Foo)         # True

# ─── hasattr ───────────────────────────────────────────────────────────────────
print(hasattr(staticmethod(lambda: 1), '__func__'))   # True
print(hasattr(classmethod(lambda cls: cls), '__func__')) # True

# ─── plain functions do not have __func__ ─────────────────────────────────────
print(hasattr(lambda: 1, '__func__'))   # False

# ─── staticmethod(f).__func__ is callable ──────────────────────────────────────
f = lambda x: x * 2
sm3 = staticmethod(f)
print(sm3.__func__(5))         # 10
