defaults = ", ".join(f"a{value}={value}" for value in range(256))
annotations = ", ".join(f"p{value}: int" for value in range(256))
base_defs = "".join(f"class B{value}: pass\n" for value in range(256))
base_names = ", ".join(f"B{value}" for value in range(256))
class_keywords = ", ".join(f"k{value}={value}" for value in range(256))
unpack_after = ", ".join(f"u{value}" for value in range(256))
match_args = ", ".join(repr(f"x{value}") for value in range(256))
match_patterns = ", ".join(f"m{value}" for value in range(256))

source = (
    f"def with_defaults({defaults}):\n    return (a0, a255)\n"
    "print('defaults', with_defaults())\n"
    f"def with_annotations({annotations}) -> int:\n    return 0\n"
    "print('annotations', len(with_annotations.__annotations__))\n"
    f"{base_defs}"
    f"class WideBases({base_names}):\n    pass\n"
    "print('bases', len(WideBases.__bases__))\n"
    "class KeywordBase:\n"
    "    def __init_subclass__(cls, **kwargs):\n"
    "        cls.keyword_count = len(kwargs)\n"
    f"class WideKeywords(KeywordBase, {class_keywords}):\n    pass\n"
    "print('keywords', WideKeywords.keyword_count)\n"
    f"*unpack_rest, {unpack_after} = range(260)\n"
    "print('unpack', unpack_rest, u0, u255)\n"
    "class PatternSubject:\n"
    f"    __match_args__ = ({match_args},)\n"
    "pattern_subject = PatternSubject()\n"
    "for pattern_index in range(256):\n"
    "    setattr(pattern_subject, 'x' + str(pattern_index), pattern_index)\n"
    "match pattern_subject:\n"
    f"    case PatternSubject({match_patterns}):\n"
    "        print('pattern', m0, m255)\n"
)

exec(source)

too_many_before = ", ".join(f"z{value}" for value in range(256)) + ", *rest = ()"
try:
    compile(too_many_before, "<wide-unpack>", "exec")
except SyntaxError as error:
    print(
        "unpack-before-error",
        type(error).__name__,
        "too many expressions in star-unpacking assignment" in str(error),
    )
