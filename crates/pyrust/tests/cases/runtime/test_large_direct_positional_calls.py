def collect(*args):
    return len(args), args[0], args[-1]


class Collector:
    def collect(self, *args):
        return len(args), args[0], args[-1]


arguments = ", ".join(str(value) for value in range(256))
namespace = {"collect": collect, "Collector": Collector}
exec(
    "print(collect(" + arguments + "))\n"
    "print(Collector().collect(" + arguments + "))",
    namespace,
)
