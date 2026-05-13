# finally's return wins
def a():
    try:
        return 1
    finally:
        return 2
print(a())                # 2

# no finally return — try's return survives
def b():
    try:
        return 1
    finally:
        pass
print(b())                # 1

# finally swallows exception when it returns
def c():
    try:
        raise ValueError("oops")
    finally:
        return 99
print(c())                # 99

# nested try/finally — innermost return wins
def d():
    try:
        try:
            return 1
        finally:
            return 2
    finally:
        return 3
print(d())                # 3

# try/return + finally with side effect, no finally-return
def e():
    try:
        return "try"
    finally:
        print("fin-ran")
print(e())
# Output: fin-ran; try
