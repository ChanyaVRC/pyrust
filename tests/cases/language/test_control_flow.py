# while/if/for/break/continue/for-else/while-else

# while + if + pass
s = 0
i = 0
while i < 5:
    if i % 2 == 0:
        s = s + i
    else:
        pass
    i = i + 1
print("sum", s)

# for + range
acc = 0
for n in range(1, 8, 2):
    acc = acc + n
print("for-range", acc)

# for + string iteration
letters = "abc"
collected = ""
for ch in letters:
    collected = collected + ch
print("for-str", collected)

# for + list iteration
for ch in ["a", "b", "c"]:
    collected = collected + ch
print("for-collection", collected)

# for + dictionary keys
d = {"a": 1, "b": 2}
key_cat = ""
for k in d:
    key_cat = key_cat + k
print("for-dict-keys", key_cat)

# for-else (when loop completes without break)
for item in []:
    print("for-else-empty-body", item)
else:
    print("for-else-empty", 1)

# for-else (when break stops the loop)
marker = "clean"
for n in [1, 2, 3]:
    if n == 2:
        marker = "break"
        break
else:
    marker = "else"
print("for-else-break", marker)

# while-else (when condition is false from start)
counter = 0
while counter < 0:
    counter = counter + 1
else:
    print("while-else-fallthrough", counter)

# while-else (with break)
counter = 0
while counter < 3:
    counter = counter + 1
    if counter == 2:
        break
else:
    print("while-else-break", "miss")
print("while-break-counter", counter)
