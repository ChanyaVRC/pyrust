unary_calls = []


class UnaryProbe:
    def __neg__(self):
        unary_calls.append(len(unary_calls) + 1)
        return unary_calls[-1]


unary_probe = UnaryProbe()
unary_first = -unary_probe
unary_second = -unary_probe
print("cse unary protocol:", unary_first, unary_second, unary_calls)


binary_calls = []


class BinaryProbe:
    def __add__(self, other):
        binary_calls.append((len(binary_calls) + 1, other))
        return binary_calls[-1][0]


binary_probe = BinaryProbe()
binary_first = binary_probe + 1
binary_second = binary_probe + 1
print("cse binary protocol:", binary_first, binary_second, binary_calls)


# A globals alias may overwrite a module fastlocal without an explicit
# register-writing opcode. A later equal LoadConst cannot reuse that named
# register as its CopyReg source.
cse_named_source = 1
cse_namespace = globals()
cse_namespace["cse_named_source"] = 2
cse_named_destination = 1
print("cse named local:", cse_named_source, cse_named_destination)
