# __delattr__ is called on `del obj.attr` when the class defines it,
# symmetric with __setattr__ on attribute assignment.

class Guarded:
    def __delattr__(self, name):
        raise AttributeError(f"deletion of '{name}' is forbidden")

g = Guarded()
g.x = 1
try:
    del g.x
except AttributeError as e:
    print(e)    # deletion of 'x' is forbidden

# Without __delattr__, normal deletion works.
class Plain:
    pass

p = Plain()
p.x = 42
del p.x
try:
    _ = p.x
except AttributeError as e:
    print(type(e).__name__)  # AttributeError

# __delattr__ receives the attribute name as a string.
class Logger:
    deleted = []
    def __delattr__(self, name):
        Logger.deleted.append(name)

lo = Logger()
lo.a = 1
lo.b = 2
del lo.a
del lo.b
print(Logger.deleted)   # ['a', 'b']

# __delattr__ inherited from base class.
class Base:
    log = []
    def __delattr__(self, name):
        Base.log.append(name)

class Child(Base):
    pass

ch = Child()
del ch.anything   # inherited __delattr__ appends 'anything'
print(Base.log)   # ['anything']
