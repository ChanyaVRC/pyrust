import math


class MF(float):
    pass


class MI(int):
    pass


print(math.sqrt(MF(4.0)))   # 2.0
print(math.pow(MF(2.0), MI(3)))  # 8.0
print(math.floor(MF(2.7)))  # 2
print(math.ceil(MF(2.3)))   # 3
