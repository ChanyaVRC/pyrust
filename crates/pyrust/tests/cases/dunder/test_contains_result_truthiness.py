# The `in` operator must apply Python's truth-value protocol to the arbitrary
# object returned by __contains__, for both instances and metaclasses.

events = []


class Decision:
    def __init__(self, label, answer):
        self.label = label
        self.answer = answer

    def __bool__(self):
        events.append(self.label)
        return self.answer


class Container:
    def __contains__(self, item):
        return Decision("instance:" + item, item == "yes")


class Meta(type):
    def __contains__(cls, item):
        return Decision("metaclass:" + str(item), item == 7)


class Target(metaclass=Meta):
    pass


print("instance", "no" in Container(), "yes" in Container())
print("metaclass", 3 in Target, 7 in Target)
print("events", events)
