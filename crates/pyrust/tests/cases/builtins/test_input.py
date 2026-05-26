# Parity fixture for input() — tests that don't require interactive stdin.
# The parity harness runs scripts with stdin closed (EOF), so happy-path tests
# that need real input are exercised manually (see commit body).

# Basic identity checks — verify input is a callable builtin.
print(callable(input))
print(type(input).__name__)

# Too many positional arguments.
try:
    input(1, 2)
except TypeError as e:
    print("TypeError:", e)

try:
    input("a", "b", "c")
except TypeError as e:
    print("TypeError:", e)

# Keyword arguments are not accepted.
try:
    input(prompt="hi")
except TypeError as e:
    print("TypeError:", e)

# EOF when stdin is exhausted (harness provides no stdin).
try:
    input()
except EOFError as e:
    print("EOFError:", e)

# EOF with a prompt — the prompt should be printed before the error.
try:
    input("Prompt: ")
except EOFError as e:
    print("EOFError:", e)
