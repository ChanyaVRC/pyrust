def early_return(x):
    if x > 0:
        return "positive"
    return "non-positive"

print(early_return(5))
print(early_return(-1))
print(early_return(0))

def nested_returns(x):
    if x > 10:
        if x > 20:
            return "very large"
        return "large"
    return "small"

print(nested_returns(25))
print(nested_returns(15))
print(nested_returns(5))
