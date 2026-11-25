# CPython 3.12 parity tests for the native `optparse` module (issue #3248).

import io
import optparse

print("version:", optparse.__version__)
print("all:", optparse.__all__)
print("make_option_is_Option:", optparse.make_option is optparse.Option)
print("constants:", optparse.SUPPRESS_HELP, optparse.SUPPRESS_USAGE, optparse.NO_DEFAULT)

# Exception hierarchy
print("exc_hierarchy:", (
    issubclass(optparse.OptParseError, Exception),
    issubclass(optparse.OptionError, optparse.OptParseError),
    issubclass(optparse.OptionConflictError, optparse.OptionError),
    issubclass(optparse.OptionValueError, optparse.OptParseError),
    issubclass(optparse.BadOptionError, optparse.OptParseError),
    issubclass(optparse.AmbiguousOptionError, optparse.BadOptionError),
))
print("exc_str:", (
    str(optparse.OptParseError("boom")),
    str(optparse.OptionError("bad", "-x")),
    str(optparse.BadOptionError("--nope")),
    str(optparse.AmbiguousOptionError("--f", ["--foo", "--foobar"])),
))

# Comprehensive OptionParser with all standard actions and types
cb_events = []

def my_callback(option, opt_str, value, parser, *args, **kwargs):
    cb_events.append((opt_str, value, args, sorted(kwargs.items())))
    setattr(parser.values, option.dest, value)

parser = optparse.OptionParser(
    usage="usage: %prog [options] arg",
    version="%prog 1.0",
    prog="demo",
    description="Demo description for %prog.",
    epilog="End of help.",
)
parser.add_option("-f", "--file", dest="filename", default="out.txt", help="write to FILE [default: %default]", metavar="FILE")
parser.add_option("-n", "--num", type="int", default=0, help="integer count")
parser.add_option("-l", "--longnum", type="long", help="long int")
parser.add_option("-r", "--ratio", type="float", default=1.5, help="float ratio")
parser.add_option("-z", "--cplx", type="complex", help="complex value")
parser.add_option("-m", "--mode", type="choice", choices=["fast", "slow"], default="fast", help="mode")
parser.add_option("-v", "--verbose", action="store_true", dest="verbose", default=False)
parser.add_option("-q", "--quiet", action="store_false", dest="verbose")
parser.add_option("-c", "--const", action="store_const", const=42, dest="answer")
parser.add_option("-I", "--include", action="append", dest="includes")
parser.add_option("-F", "--flag", action="append_const", const="X", dest="flags")
parser.add_option("-d", "--debug", action="count", dest="debug", default=0)
parser.add_option("-p", "--pair", type="int", nargs=2, dest="pair")
parser.add_option("-x", "--extra", action="callback", callback=my_callback, type="string", dest="extra", callback_args=(1,), callback_kwargs={"k": "v"})
parser.add_option("--secret", help=optparse.SUPPRESS_HELP)

grp = optparse.OptionGroup(parser, "Advanced Options", "Extra settings.")
grp.add_option("--level", type="int", default=3, help="level number")
parser.add_option_group(grp)
parser.set_defaults(extra_default="ok")

opts, args = parser.parse_args([
    "-f", "a.txt",
    "--num=0x10",
    "--longnum", "010",
    "-r2.25",
    "--cplx", "1+2j",
    "-m", "slow",
    "-v",
    "-c",
    "-I", "dir1", "--include=dir2",
    "-FF",
    "-ddd",
    "-p", "10", "20",
    "--ex=hello",
    "--level", "9",
    "pos1",
    "--",
    "--not-an-opt",
])

print("parsed_opts:", (
    opts.filename,
    opts.num,
    opts.longnum,
    opts.ratio,
    opts.cplx,
    opts.mode,
    opts.verbose,
    opts.answer,
    opts.includes,
    opts.flags,
    opts.debug,
    opts.pair,
    opts.extra,
    opts.level,
    opts.extra_default,
))
print("parsed_args:", args)
print("cb_events:", cb_events)
print("values_eq:", opts == opts, opts == vars(opts))

# Help formatting (IndentedHelpFormatter & TitledHelpFormatter)
print("--- indented help ---")
print(parser.format_help(), end="")

titled = optparse.OptionParser(
    prog="demo",
    formatter=optparse.TitledHelpFormatter(),
    description="Titled demo.",
)
titled.add_option("-a", "--alpha", help="alpha opt")
print("--- titled help ---")
print(titled.format_help(), end="")

# Conflict handling: error vs resolve
p_err = optparse.OptionParser(add_help_option=False)
p_err.add_option("-a", "--alpha")
try:
    p_err.add_option("-a", "--alt")
    print("UNEXPECTED_NO_CONFLICT")
except optparse.OptionConflictError as e:
    print("conflict_error:", str(e))

p_res = optparse.OptionParser(conflict_handler="resolve", add_help_option=False)
p_res.add_option("-a", "--alpha")
p_res.add_option("-a", "--alt")
print("conflict_resolved:", p_res.has_option("-a"), p_res.has_option("--alpha"), p_res.has_option("--alt"))

# Disabled interspersed args
p_no_inter = optparse.OptionParser(add_help_option=False)
p_no_inter.disable_interspersed_args()
p_no_inter.add_option("-a", action="store_true")
o2, a2 = p_no_inter.parse_args(["pos", "-a"])
print("no_interspersed:", o2.a, a2)

# Custom error override & invalid value / unknown option / ambiguous option
class QuietParser(optparse.OptionParser):
    def error(self, msg):
        raise optparse.OptParseError(msg)

qp = QuietParser(prog="q", add_help_option=False)
qp.add_option("--foo", type="int")
qp.add_option("--foobar", type="choice", choices=["a", "b"])
for bad_argv in [["--foo", "abc"], ["--foobar", "z"], ["--f"], ["--unknown"]]:
    try:
        qp.parse_args(bad_argv)
        print("UNEXPECTED_OK", bad_argv)
    except optparse.OptParseError as e:
        print("error_msg:", bad_argv, str(e))
