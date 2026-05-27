# type() of enumerate and zip objects must report the correct class name.
print(type(enumerate([])).__name__)       # enumerate
print(type(zip([],[])).__name__)          # zip
print(type(enumerate(range(3))).__name__) # enumerate
print(type(zip([1],[2])).__name__)        # zip
