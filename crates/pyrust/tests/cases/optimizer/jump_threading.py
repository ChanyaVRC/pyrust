def classify(x):
    if x > 0:
        if x > 100:
            return "large"
        else:
            return "positive"
    else:
        return "non-positive"

print(classify(150))
print(classify(50))
print(classify(-5))
print(classify(0))

result = []
for i in range(5):
    if i % 2 == 0:
        result.append("even")
    else:
        result.append("odd")
print(result)
