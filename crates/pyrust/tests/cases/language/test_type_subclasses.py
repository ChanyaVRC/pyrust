# Test type.__subclasses__() — returns list of direct subclasses.

class Animal: pass
class Dog(Animal): pass
class Cat(Animal): pass
class GuideDog(Dog): pass

# Direct subclasses of Animal
subs = Animal.__subclasses__()
print(sorted([c.__name__ for c in subs]))

# Direct subclasses of Dog (not Animal's transitive ones)
dog_subs = Dog.__subclasses__()
print([c.__name__ for c in dog_subs])

# A leaf class has no subclasses
print(Cat.__subclasses__())

# Multiple inheritance: class registers with all bases
class Flyable: pass
class FlyingDog(Dog, Flyable): pass

print([c.__name__ for c in Dog.__subclasses__()])
print([c.__name__ for c in Flyable.__subclasses__()])

# type(name, bases, dict) dynamic class also registers
DynChild = type("DynChild", (Animal,), {})
print("DynChild" in [c.__name__ for c in Animal.__subclasses__()])

# __subclasses__() takes no arguments
try:
    Animal.__subclasses__(1)
except TypeError as e:
    print(type(e).__name__)

try:
    Animal.__subclasses__(extra=1)
except TypeError as e:
    print(type(e).__name__)
