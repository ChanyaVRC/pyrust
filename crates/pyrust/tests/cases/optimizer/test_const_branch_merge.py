# A ternary's two arms converge on one truth test.  The optimizer must not
# treat the linearly adjacent else-arm constant as a dominating definition.


for flag in (True, False):
    events = []
    if True if flag else False:
        events.append("body")
    print(flag, events)


for flag in (True, False):
    events = []
    if False if flag else True:
        events.append("body")
    print(flag, events)


# The same merge rule applies when the linearly adjacent else arm ends in
# UnaryOp(Not).  not-inversion must not replace the shared test with a test of
# that arm's source register.
for flag in (True, False):
    x = True
    y = True
    events = []
    if y if flag else not x:
        events.append("body")
    print("not", flag, events)


for flag in (True, False):
    print("tuple", flag, (10 if flag else 20,))
