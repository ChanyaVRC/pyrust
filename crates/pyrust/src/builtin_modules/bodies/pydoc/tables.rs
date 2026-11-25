// Static tables for `pydoc` (exact CPython 3.12 parity).

const MODULE_DOC: &str = "\
Generate Python documentation in HTML or text for interactive use.

At the Python interactive prompt, calling help(thing) on a Python object
documents the object, and calling help() starts up an interactive
help session.

Or, at the shell command line outside of Python:

Run \"pydoc <name>\" to show documentation on something.  <name> may be
the name of a function, module, package, or a dotted reference to a
class or function within a module or module in a package.  If the
argument contains a path segment delimiter (e.g. slash on Unix,
backslash on Windows) it is treated as the path to a Python source file.

Run \"pydoc -k <keyword>\" to search for a keyword in the synopsis lines
of all available modules.

Run \"pydoc -w <name>\" to write out the HTML documentation for a module
to a file named \"<name>.html\".
";

const MODULE_CREDITS: &str = "\
Guido van Rossum, for an excellent programming language.
Tommy Burnette, the original creator of manpy.
Paul Prescod, for all his work on onlinehelp.
Richard Chamberlain, for the first implementation of textdoc.
";

const HIDDEN_DUNDERS: &[&str] = &[
    "__author__",
    "__builtins__",
    "__cached__",
    "__credits__",
    "__date__",
    "__doc__",
    "__file__",
    "__spec__",
    "__loader__",
    "__module__",
    "__name__",
    "__package__",
    "__path__",
    "__qualname__",
    "__slots__",
    "__version__",
];

const STDLIB_BUILTIN_DOC_MODULES: &[&str] = &[
    "errno",
    "exceptions",
    "gc",
    "marshal",
    "posix",
    "signal",
    "sys",
    "_thread",
    "zipimport",
];

enum TopicEntry {
    Alias(&'static str),
    Target(&'static str, &'static str),
}

const HELPER_KEYWORDS: &[(&str, TopicEntry)] = &[
    ("False", TopicEntry::Alias("")),
    ("None", TopicEntry::Alias("")),
    ("True", TopicEntry::Alias("")),
    ("and", TopicEntry::Alias("BOOLEAN")),
    ("as", TopicEntry::Alias("with")),
    ("assert", TopicEntry::Target("assert", "")),
    ("async", TopicEntry::Target("async", "")),
    ("await", TopicEntry::Target("await", "")),
    ("break", TopicEntry::Target("break", "while for")),
    (
        "class",
        TopicEntry::Target("class", "CLASSES SPECIALMETHODS"),
    ),
    ("continue", TopicEntry::Target("continue", "while for")),
    ("def", TopicEntry::Target("function", "")),
    ("del", TopicEntry::Target("del", "BASICMETHODS")),
    ("elif", TopicEntry::Alias("if")),
    ("else", TopicEntry::Target("else", "while for")),
    ("except", TopicEntry::Alias("try")),
    ("finally", TopicEntry::Alias("try")),
    ("for", TopicEntry::Target("for", "break continue while")),
    ("from", TopicEntry::Alias("import")),
    (
        "global",
        TopicEntry::Target("global", "nonlocal NAMESPACES"),
    ),
    ("if", TopicEntry::Target("if", "TRUTHVALUE")),
    ("import", TopicEntry::Target("import", "MODULES")),
    ("in", TopicEntry::Target("in", "SEQUENCEMETHODS")),
    ("is", TopicEntry::Alias("COMPARISON")),
    ("lambda", TopicEntry::Target("lambda", "FUNCTIONS")),
    (
        "nonlocal",
        TopicEntry::Target("nonlocal", "global NAMESPACES"),
    ),
    ("not", TopicEntry::Alias("BOOLEAN")),
    ("or", TopicEntry::Alias("BOOLEAN")),
    ("pass", TopicEntry::Target("pass", "")),
    ("raise", TopicEntry::Target("raise", "EXCEPTIONS")),
    ("return", TopicEntry::Target("return", "FUNCTIONS")),
    ("try", TopicEntry::Target("try", "EXCEPTIONS")),
    (
        "while",
        TopicEntry::Target("while", "break continue if TRUTHVALUE"),
    ),
    (
        "with",
        TopicEntry::Target("with", "CONTEXTMANAGERS EXCEPTIONS yield"),
    ),
    ("yield", TopicEntry::Target("yield", "")),
];

const HELPER_SYMBOLS: &[(&str, &str)] = &[
    ("%", "OPERATORS FORMATTING"),
    ("**", "POWER OPERATORS"),
    (",", "TUPLES LISTS FUNCTIONS"),
    (".", "ATTRIBUTES FLOAT MODULES OBJECTS"),
    ("...", "ELLIPSIS"),
    (":", "SLICINGS DICTIONARYLITERALS"),
    ("@", "def class"),
    ("\\", "STRINGS"),
    ("_", "PRIVATENAMES"),
    ("__", "PRIVATENAMES SPECIALMETHODS"),
    ("`", "BACKQUOTES"),
    ("(", "TUPLES FUNCTIONS CALLS"),
    (")", "TUPLES FUNCTIONS CALLS"),
    ("[", "LISTS SUBSCRIPTS SLICINGS"),
    ("]", "LISTS SUBSCRIPTS SLICINGS"),
    ("'", "STRINGS"),
    ("'''", "STRINGS"),
    ("\"", "STRINGS"),
    ("\"\"\"", "STRINGS"),
    ("b'", "STRINGS"),
    ("b\"", "STRINGS"),
    ("f'", "STRINGS"),
    ("f\"", "STRINGS"),
    ("r'", "STRINGS"),
    ("r\"", "STRINGS"),
    ("u'", "STRINGS"),
    ("u\"", "STRINGS"),
    ("+", "OPERATORS"),
    ("-", "OPERATORS UNARY"),
    ("*", "OPERATORS"),
    ("/", "OPERATORS"),
    ("//", "OPERATORS"),
    ("<<", "OPERATORS BITWISE"),
    (">>", "OPERATORS BITWISE"),
    ("&", "OPERATORS BITWISE"),
    ("|", "OPERATORS BITWISE"),
    ("^", "OPERATORS BITWISE"),
    ("~", "OPERATORS UNARY BITWISE"),
    ("<", "OPERATORS COMPARISON"),
    (">", "OPERATORS COMPARISON"),
    ("<=", "OPERATORS COMPARISON"),
    (">=", "OPERATORS COMPARISON"),
    ("==", "OPERATORS COMPARISON"),
    ("!=", "OPERATORS COMPARISON"),
    ("<>", "OPERATORS COMPARISON"),
    ("+=", "AUGMENTEDASSIGNMENT"),
    ("-=", "AUGMENTEDASSIGNMENT"),
    ("*=", "AUGMENTEDASSIGNMENT"),
    ("/=", "AUGMENTEDASSIGNMENT"),
    ("%=", "AUGMENTEDASSIGNMENT"),
    ("&=", "AUGMENTEDASSIGNMENT"),
    ("|=", "AUGMENTEDASSIGNMENT"),
    ("^=", "AUGMENTEDASSIGNMENT"),
    ("<<=", "AUGMENTEDASSIGNMENT"),
    (">>=", "AUGMENTEDASSIGNMENT"),
    ("**=", "AUGMENTEDASSIGNMENT"),
    ("//=", "AUGMENTEDASSIGNMENT"),
    ("j", "COMPLEX"),
    ("J", "COMPLEX"),
];

const HELPER_TOPICS: &[(&str, TopicEntry)] = &[
    (
        "TYPES",
        TopicEntry::Target(
            "types",
            "STRINGS UNICODE NUMBERS SEQUENCES MAPPINGS FUNCTIONS CLASSES MODULES FILES inspect",
        ),
    ),
    (
        "STRINGS",
        TopicEntry::Target(
            "strings",
            "str UNICODE SEQUENCES STRINGMETHODS FORMATTING TYPES",
        ),
    ),
    (
        "STRINGMETHODS",
        TopicEntry::Target("string-methods", "STRINGS FORMATTING"),
    ),
    (
        "FORMATTING",
        TopicEntry::Target("formatstrings", "OPERATORS"),
    ),
    (
        "UNICODE",
        TopicEntry::Target(
            "strings",
            "encodings unicode SEQUENCES STRINGMETHODS FORMATTING TYPES",
        ),
    ),
    (
        "NUMBERS",
        TopicEntry::Target("numbers", "INTEGER FLOAT COMPLEX TYPES"),
    ),
    ("INTEGER", TopicEntry::Target("integers", "int range")),
    ("FLOAT", TopicEntry::Target("floating", "float math")),
    ("COMPLEX", TopicEntry::Target("imaginary", "complex cmath")),
    (
        "SEQUENCES",
        TopicEntry::Target("typesseq", "STRINGMETHODS FORMATTING range LISTS"),
    ),
    ("MAPPINGS", TopicEntry::Alias("DICTIONARIES")),
    (
        "FUNCTIONS",
        TopicEntry::Target("typesfunctions", "def TYPES"),
    ),
    (
        "METHODS",
        TopicEntry::Target("typesmethods", "class def CLASSES TYPES"),
    ),
    (
        "CODEOBJECTS",
        TopicEntry::Target("bltin-code-objects", "compile FUNCTIONS TYPES"),
    ),
    (
        "TYPEOBJECTS",
        TopicEntry::Target("bltin-type-objects", "types TYPES"),
    ),
    ("FRAMEOBJECTS", TopicEntry::Alias("TYPES")),
    ("TRACEBACKS", TopicEntry::Alias("TYPES")),
    ("NONE", TopicEntry::Target("bltin-null-object", "")),
    (
        "ELLIPSIS",
        TopicEntry::Target("bltin-ellipsis-object", "SLICINGS"),
    ),
    ("SPECIALATTRIBUTES", TopicEntry::Target("specialattrs", "")),
    (
        "CLASSES",
        TopicEntry::Target("types", "class SPECIALMETHODS PRIVATENAMES"),
    ),
    ("MODULES", TopicEntry::Target("typesmodules", "import")),
    ("PACKAGES", TopicEntry::Alias("import")),
    (
        "EXPRESSIONS",
        TopicEntry::Target(
            "operator-summary",
            "lambda or and not in is BOOLEAN COMPARISON BITWISE SHIFTING BINARY FORMATTING POWER UNARY ATTRIBUTES SUBSCRIPTS SLICINGS CALLS TUPLES LISTS DICTIONARIES",
        ),
    ),
    ("OPERATORS", TopicEntry::Alias("EXPRESSIONS")),
    ("PRECEDENCE", TopicEntry::Alias("EXPRESSIONS")),
    ("OBJECTS", TopicEntry::Target("objects", "TYPES")),
    (
        "SPECIALMETHODS",
        TopicEntry::Target(
            "specialnames",
            "BASICMETHODS ATTRIBUTEMETHODS CALLABLEMETHODS SEQUENCEMETHODS MAPPINGMETHODS NUMBERMETHODS CLASSES",
        ),
    ),
    (
        "BASICMETHODS",
        TopicEntry::Target("customization", "hash repr str SPECIALMETHODS"),
    ),
    (
        "ATTRIBUTEMETHODS",
        TopicEntry::Target("attribute-access", "ATTRIBUTES SPECIALMETHODS"),
    ),
    (
        "CALLABLEMETHODS",
        TopicEntry::Target("callable-types", "CALLS SPECIALMETHODS"),
    ),
    (
        "SEQUENCEMETHODS",
        TopicEntry::Target("sequence-types", "SEQUENCES SEQUENCEMETHODS SPECIALMETHODS"),
    ),
    (
        "MAPPINGMETHODS",
        TopicEntry::Target("sequence-types", "MAPPINGS SPECIALMETHODS"),
    ),
    (
        "NUMBERMETHODS",
        TopicEntry::Target(
            "numeric-types",
            "NUMBERS AUGMENTEDASSIGNMENT SPECIALMETHODS",
        ),
    ),
    (
        "EXECUTION",
        TopicEntry::Target("execmodel", "NAMESPACES DYNAMICFEATURES EXCEPTIONS"),
    ),
    (
        "NAMESPACES",
        TopicEntry::Target(
            "naming",
            "global nonlocal ASSIGNMENT DELETION DYNAMICFEATURES",
        ),
    ),
    (
        "DYNAMICFEATURES",
        TopicEntry::Target("dynamic-features", ""),
    ),
    ("SCOPING", TopicEntry::Alias("NAMESPACES")),
    ("FRAMES", TopicEntry::Alias("NAMESPACES")),
    (
        "EXCEPTIONS",
        TopicEntry::Target("exceptions", "try except finally raise"),
    ),
    ("CONVERSIONS", TopicEntry::Target("conversions", "")),
    (
        "IDENTIFIERS",
        TopicEntry::Target("identifiers", "keywords SPECIALIDENTIFIERS"),
    ),
    ("SPECIALIDENTIFIERS", TopicEntry::Target("id-classes", "")),
    ("PRIVATENAMES", TopicEntry::Target("atom-identifiers", "")),
    (
        "LITERALS",
        TopicEntry::Target(
            "atom-literals",
            "STRINGS NUMBERS TUPLELITERALS LISTLITERALS DICTIONARYLITERALS",
        ),
    ),
    ("TUPLES", TopicEntry::Alias("SEQUENCES")),
    (
        "TUPLELITERALS",
        TopicEntry::Target("exprlists", "TUPLES LITERALS"),
    ),
    (
        "LISTS",
        TopicEntry::Target("typesseq-mutable", "LISTLITERALS"),
    ),
    (
        "LISTLITERALS",
        TopicEntry::Target("lists", "LISTS LITERALS"),
    ),
    (
        "DICTIONARIES",
        TopicEntry::Target("typesmapping", "DICTIONARYLITERALS"),
    ),
    (
        "DICTIONARYLITERALS",
        TopicEntry::Target("dict", "DICTIONARIES LITERALS"),
    ),
    (
        "ATTRIBUTES",
        TopicEntry::Target(
            "attribute-references",
            "getattr hasattr setattr ATTRIBUTEMETHODS",
        ),
    ),
    (
        "SUBSCRIPTS",
        TopicEntry::Target("subscriptions", "SEQUENCEMETHODS"),
    ),
    (
        "SLICINGS",
        TopicEntry::Target("slicings", "SEQUENCEMETHODS"),
    ),
    ("CALLS", TopicEntry::Target("calls", "EXPRESSIONS")),
    ("POWER", TopicEntry::Target("power", "EXPRESSIONS")),
    ("UNARY", TopicEntry::Target("unary", "EXPRESSIONS")),
    ("BINARY", TopicEntry::Target("binary", "EXPRESSIONS")),
    ("SHIFTING", TopicEntry::Target("shifting", "EXPRESSIONS")),
    ("BITWISE", TopicEntry::Target("bitwise", "EXPRESSIONS")),
    (
        "COMPARISON",
        TopicEntry::Target("comparisons", "EXPRESSIONS BASICMETHODS"),
    ),
    (
        "BOOLEAN",
        TopicEntry::Target("booleans", "EXPRESSIONS TRUTHVALUE"),
    ),
    ("ASSERTION", TopicEntry::Alias("assert")),
    (
        "ASSIGNMENT",
        TopicEntry::Target("assignment", "AUGMENTEDASSIGNMENT"),
    ),
    (
        "AUGMENTEDASSIGNMENT",
        TopicEntry::Target("augassign", "NUMBERMETHODS"),
    ),
    ("DELETION", TopicEntry::Alias("del")),
    ("RETURNING", TopicEntry::Alias("return")),
    ("IMPORTING", TopicEntry::Alias("import")),
    ("CONDITIONAL", TopicEntry::Alias("if")),
    (
        "LOOPING",
        TopicEntry::Target("compound", "for while break continue"),
    ),
    (
        "TRUTHVALUE",
        TopicEntry::Target("truth", "if while and or not BASICMETHODS"),
    ),
    ("DEBUGGING", TopicEntry::Target("debugger", "pdb")),
    (
        "CONTEXTMANAGERS",
        TopicEntry::Target("context-managers", "with"),
    ),
];

fn build_topic_map(entries: &[(&str, TopicEntry)]) -> Value {
    let mut d = PyDict::default();
    for (k, entry) in entries {
        let val = match entry {
            TopicEntry::Alias(s) => Value::string(*s),
            TopicEntry::Target(label, xrefs) => {
                Value::tuple(vec![Value::string(*label), Value::string(*xrefs)])
            }
        };
        d.insert(PyKey::str_from(k), val);
    }
    Value::dict(d)
}

fn build_helper_keywords() -> Value {
    build_topic_map(HELPER_KEYWORDS)
}

fn build_helper_topics() -> Value {
    build_topic_map(HELPER_TOPICS)
}

fn build_helper_symbols() -> Value {
    let mut d = PyDict::default();
    for &(k, v) in HELPER_SYMBOLS {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    Value::dict(d)
}
