class A:
    @classmethod
    def greet(cls):
        return f"A.{cls.__name__}"


class B(A):
    @classmethod
    def greet(cls):
        return super().greet()

    def run(self):
        return super().greet()


class C(B):
    pass


# super().classmethod() from a classmethod
print(B.greet())
print(C.greet())

# super().classmethod() from an instance method binds type(instance)
print(B().run())
print(C().run())
print(B().greet())
print(C().greet())
