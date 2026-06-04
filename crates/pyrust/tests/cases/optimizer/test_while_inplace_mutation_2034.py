# Issue #2034: `while <container>:` whose body mutates the container *in place*
# must re-evaluate the condition every iteration.  Previously LICM treated the
# bare-name truthiness as loop-invariant (the register is never reassigned) and
# hoisted the check out of the loop, causing one extra iteration and a crash.

# list: drain via pop()
stack = [1, 2, 3]
n = 0
while stack:
    stack.pop()
    n += 1
print(n, stack)

# list: drain via pop(0)
queue = [10, 20, 30, 40]
while queue:
    queue.pop(0)
print(queue)

# set: drain via pop()
s = {1, 2, 3}
while s:
    s.pop()
print(s)

# dict: drain via popitem()
d = {"a": 1, "b": 2, "c": 3}
while d:
    d.popitem()
print(d)

# aliasing: the mutation goes through a different name that aliases the object
a = [1, 2, 3]
b = a
while a:
    b.pop()
print(a, b)

# subscript-delete mutation
lst = [1, 2, 3]
while lst:
    del lst[-1]
print(lst)

# clear() empties in one shot
items = [1, 2, 3]
while items:
    items.clear()
print(items)

# nested condition expression involving the mutated container
bucket = [1, 2, 3, 4, 5]
while len(bucket) > 0 and bucket:
    bucket.pop()
print(bucket)

# genuinely loop-invariant flag condition still terminates correctly
done = False
count = 0
while not done:
    count += 1
    if count >= 10:
        break
print(count)
