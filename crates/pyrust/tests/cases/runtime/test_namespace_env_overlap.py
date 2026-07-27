# A nested same-root exec can create a slow EnvValues binding for a name that
# the suspended outer Script also owns as a fastlocal. A later outer assignment
# must refresh both representations so LoadGlobal never lets the stale slow
# binding win over the authoritative register.
overlap_value = 1
exec("global overlap_value\noverlap_value = 2")
overlap_value = 3
exec("def read_overlap():\n    return overlap_value")
print("namespace env overlap:", overlap_value, read_overlap())


# SyncModuleGlobal cannot be moved out of a loop merely because no explicit
# Call opcode is present. Binary dispatch may run user dunder code, which can
# expose and read the live globals provider on every iteration.
seen = []


class Probe:
    def __radd__(self, left):
        seen.append(globals()["sunk_value"])
        return left + 1


probe = Probe()
exec(
    "sunk_value = 0\n"
    "sunk_index = 0\n"
    "while sunk_index < 3:\n"
    "    sunk_value = sunk_value + probe\n"
    "    sunk_index += 1\n"
)
print("reentrant module sync:", seen)


# A constant-loop fold must not reuse an old LoadConst across a call. The call
# can mutate the same module fastlocal through Python's global statement.
folded_value = 0


def mutate_folded_value():
    global folded_value
    folded_value = 100


mutate_folded_value()
for folded_index in range(3):
    folded_value += 1
print("reentrant linear fold:", folded_value)
