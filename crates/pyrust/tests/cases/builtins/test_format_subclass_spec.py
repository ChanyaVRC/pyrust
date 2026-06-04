# Parity fixture for format()/f-string on built-in subclasses with a spec
# (issue #1935).  format(MyInt(5), 'x') and f"{MyInt(5):x}" must extract the
# primitive backing and format it (the same way str.format already does),
# rather than invoking the inherited builtin __format__ which rejects a
# non-empty spec.  A subclass that *overrides* __format__ uses the override.

class I(int):
    pass

class F(float):
    pass

class S(str):
    pass

class B(bytes):
    pass

# --- format() builtin with spec extracts backing ---
print(repr(format(I(42), '05d')))
print(repr(format(I(255), 'x')))
print(repr(format(F(3.14159), '.2f')))
print(repr(format(S('hi'), '>5')))

# --- f-string with spec ---
print(repr(f"{I(42):05d}"))
print(repr(f"{F(3.14159):.2f}"))
print(repr(f"{S('hi'):>5}"))

# --- str.format path agrees (was already correct) ---
print(repr("{:05d}".format(I(42))))
print(repr("{:.2f}".format(F(3.14159))))

# --- empty spec works in every path ---
print(repr(format(F(3.5), '')))
print(repr(f"{I(7)}"))
print(repr("{}".format(S('hey'))))

# --- bytes subclass: empty spec ok, non-empty spec rejected (object.__format__) ---
# (The exact type name in the message — `B` vs `bytes` — is governed by a
# separate pre-existing backing-name defect, so only the raised class is
# asserted here.)
print(repr(format(B(b'hi'), '')))
try:
    format(B(b'hi'), 'x')
except TypeError:
    print('TypeError raised for bytes-subclass non-empty spec')

# --- subclass that OVERRIDES __format__ uses the override ---
class O(int):
    def __format__(self, spec):
        return 'OVERRIDE:' + spec

print(repr(format(O(5), 'abc')))
print(repr(f"{O(5):xyz}"))
print(repr(format(O(5), '')))

# --- override that delegates via super().__format__('') (empty spec) ---
class Wrapped(int):
    def __format__(self, spec):
        if spec == '':
            return 'W(' + super().__format__(spec) + ')'
        return super().__format__(spec)

print(repr(format(Wrapped(9), '')))

# --- pure user class without __format__: empty spec -> str(self), non-empty -> TypeError ---
class P:
    def __str__(self):
        return 'P-str'

print(repr(format(P(), '')))
try:
    format(P(), 'x')
except TypeError as e:
    print('TypeError:', e)

# --- pure user class WITH __format__ override is dispatched ---
class Q:
    def __format__(self, spec):
        return 'Q:' + spec

print(repr(format(Q(), 'spec')))
print(repr(f"{Q():zzz}"))
