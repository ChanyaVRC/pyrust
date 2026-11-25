// `pydoc_data.topics` module — 100% native Rust implementation of Python 3.12's
// help topic dictionary (issue #3255).
//
// Contains all 79 standard CPython 3.12 topic keys referenced by
// `pydoc.Helper.topics`, `pydoc.Helper.keywords`, and `pydoc.Helper.symbols`.

use pyrust_derive::pyrust_module;

use crate::value::{PyDict, PyKey, Value};

const TOPICS_DATA: &[(&str, &str)] = &[
    (
        "assert",
        "The \"assert\" statement\n\
**********************\n\
\n\
Assert statements are a convenient way to insert debugging assertions\n\
into a program:\n\
\n\
   assert_stmt ::= \"assert\" expression [\",\" expression]\n\
\n\
The simple form, \"assert expression\", is equivalent to:\n\
\n\
   if __debug__:\n\
       if not expression: raise AssertionError\n\
\n\
The extended form, \"assert expression1, expression2\", is equivalent to:\n\
\n\
   if __debug__:\n\
       if not expression1: raise AssertionError(expression2)\n\
\n\
The built-in variable \"__debug__\" is True under normal circumstances and\n\
False when optimization is requested (command line option \"-O\").\n",
    ),
    (
        "assignment",
        "Assignment statements\n\
*********************\n\
\n\
Assignment statements are used to (re)bind names to values and to modify\n\
attributes or items of mutable objects:\n\
\n\
   assignment_stmt ::= (target_list \"=\")+ (starred_expression | yield_expression)\n\
   target_list     ::= target (\",\" target)* [\",\"]\n\
\n\
An assignment statement evaluates the right-hand expression list and\n\
assigns the resulting object to each target list from left to right.\n\
Sequence unpacking binds targets element-by-element; a single starred\n\
target captures a list of zero or more remaining items.\n",
    ),
    (
        "async",
        "Coroutines\n\
**********\n\
\n\
Coroutine function definition:\n\
\n\
   async_funcdef ::= [decorators] \"async\" \"def\" funcname \"(\" [parameter_list] \")\" [\"->\" expression] \":\" suite\n\
\n\
Functions defined with \"async def\" are always coroutine functions (or\n\
asynchronous generator functions if they contain \"yield\"). Calling a\n\
coroutine function returns a coroutine object without executing its body\n\
until awaited or scheduled on an event loop.\n\
\n\
\"async for\" iterates over asynchronous iterables implementing \"__aiter__\"\n\
and \"__anext__\"; \"async with\" enters asynchronous context managers\n\
implementing \"__aenter__\" and \"__aexit__\".\n",
    ),
    (
        "atom-identifiers",
        "Identifiers (Names)\n\
*******************\n\
\n\
An identifier occurring as an atom is a name. When bound to an object,\n\
evaluating the atom yields that object. When unbound, evaluating it\n\
raises NameError (or UnboundLocalError for unbound local variables).\n\
\n\
Private name mangling: identifiers occurring in a class definition that\n\
begin with at least two underscores and do not end with two or more\n\
underscores (\"__name\") are textually transformed to \"_ClassName__name\".\n",
    ),
    (
        "atom-literals",
        "Literals\n\
********\n\
\n\
Python supports string and bytes literals and various numeric literals:\n\
\n\
   literal ::= stringliteral | bytesliteral | integer | floatnumber | imagnumber\n\
\n\
Evaluation of a literal yields an object of the given type (str, bytes,\n\
int, float, or complex) with the given value.\n",
    ),
    (
        "attribute-access",
        "Customizing attribute access\n\
****************************\n\
\n\
Classes can customize attribute lookup, assignment, and deletion by\n\
defining special methods:\n\
\n\
- \"object.__getattr__(self, name)\": fallback called when default\n\
  attribute lookup raises AttributeError.\n\
- \"object.__getattribute__(self, name)\": unconditionally called for\n\
  every attribute access on instances of the class.\n\
- \"object.__setattr__(self, name, value)\": called on attribute assignment.\n\
- \"object.__delattr__(self, name)\": called on \"del obj.name\".\n\
- \"object.__dir__(self)\": customizes the list returned by \"dir(obj)\".\n\
\n\
Descriptors define \"__get__\", \"__set__\", and/or \"__delete__\" to control\n\
attribute binding on owner classes.\n",
    ),
    (
        "attribute-references",
        "Attribute references\n\
********************\n\
\n\
An attribute reference is a primary followed by a period and a name:\n\
\n\
   attributeref ::= primary \".\" identifier\n\
\n\
The primary is evaluated to an object, which is then asked to produce\n\
the attribute with the given name. If the attribute is not available,\n\
AttributeError is raised.\n",
    ),
    (
        "augassign",
        "Augmented assignment statements\n\
*******************************\n\
\n\
Augmented assignment combines a binary operation and an assignment in a\n\
single statement:\n\
\n\
   augmented_assignment_stmt ::= augtarget augop (expression_list | yield_expression)\n\
   augop ::= \"+=\" | \"-=\" | \"*=\" | \"@=\" | \"/=\" | \"//=\" | \"%=\" | \"**=\"\n\
           | \">>=\" | \"<<=\" | \"&=\" | \"^=\" | \"|=\"\n\
\n\
The target is evaluated only once. When possible, the operation is\n\
performed in place via \"__iadd__\", \"__isub__\", etc., falling back to the\n\
regular binary operation and rebinding the target.\n",
    ),
    (
        "await",
        "Await expression\n\
****************\n\
\n\
   await_expr ::= \"await\" primary\n\
\n\
Suspends execution of the current coroutine until an awaitable object\n\
completes and returns its result. \"await\" is valid only inside an\n\
\"async def\" function. An awaitable is a coroutine or an object defining\n\
\"__await__()\" returning an iterator.\n",
    ),
    (
        "binary",
        "Binary arithmetic operations\n\
****************************\n\
\n\
The binary arithmetic operators have conventional priority levels:\n\
\n\
   m_expr ::= u_expr | m_expr \"*\" u_expr | m_expr \"@\" u_expr\n\
            | m_expr \"/\" u_expr | m_expr \"//\" u_expr | m_expr \"%\" u_expr\n\
   a_expr ::= m_expr | a_expr \"+\" m_expr | a_expr \"-\" m_expr\n\
\n\
\"*\" multiplies numbers or repeats sequences; \"@\" performs matrix\n\
multiplication; \"/\" performs true division; \"//\" performs floor division;\n\
\"%\" computes remainder (or printf-style string formatting); \"+\" adds\n\
numbers or concatenates sequences; \"-\" subtracts numbers or sets.\n",
    ),
    (
        "bitwise",
        "Binary bitwise operations\n\
*************************\n\
\n\
Each of the three bitwise operations has a different priority level:\n\
\n\
   and_expr ::= shift_expr | and_expr \"&\" shift_expr\n\
   xor_expr ::= and_expr | xor_expr \"^\" and_expr\n\
   or_expr  ::= xor_expr | or_expr \"|\" xor_expr\n\
\n\
On integers, \"&\" computes bitwise AND, \"^\" computes bitwise XOR, and \"|\"\n\
computes bitwise OR. Sets also support these operators for intersection,\n\
symmetric difference, and union.\n",
    ),
    (
        "bltin-code-objects",
        "Code Objects\n\
************\n\
\n\
Code objects represent byte-compiled executable Python code, or bytecode.\n\
They are returned by the built-in \"compile()\" function and accessible as\n\
the \"__code__\" attribute of function objects. Key read-only attributes\n\
include \"co_name\", \"co_qualname\", \"co_filename\", \"co_firstlineno\",\n\
\"co_argcount\", \"co_posonlyargcount\", \"co_kwonlyargcount\", \"co_varnames\",\n\
\"co_consts\", \"co_names\", \"co_freevars\", \"co_cellvars\", and \"co_flags\".\n",
    ),
    (
        "bltin-ellipsis-object",
        "The Ellipsis Object\n\
*******************\n\
\n\
This object is commonly used by slicing and type annotations. It supports\n\
no special operations. There is exactly one ellipsis object, named\n\
\"Ellipsis\" (a built-in name), and it can also be written as the literal\n\
\"...\". \"type(Ellipsis)()\" returns the \"Ellipsis\" singleton.\n",
    ),
    (
        "bltin-null-object",
        "The Null Object\n\
***************\n\
\n\
This object is returned by functions that do not explicitly return a\n\
value. It supports no special operations. There is exactly one null\n\
object, named \"None\" (a built-in name). \"type(None)()\" produces the same\n\
singleton. Its truth value is always False.\n",
    ),
    (
        "bltin-type-objects",
        "Type Objects\n\
************\n\
\n\
Type objects represent the various object types. An object's type is\n\
accessed by the built-in function \"type()\". Type objects support\n\
instantiation, attribute lookup, subclass checks (\"issubclass\"), and\n\
expose special attributes such as \"__name__\", \"__qualname__\", \"__bases__\",\n\
\"__mro__\", and \"__subclasses__()\".\n",
    ),
    (
        "booleans",
        "Boolean operations\n\
******************\n\
\n\
   or_test  ::= and_test | or_test \"or\" and_test\n\
   and_test ::= not_test | and_test \"and\" not_test\n\
   not_test ::= comparison | \"not\" not_test\n\
\n\
\"not\" yields True if its argument is false, False otherwise. \"x and y\"\n\
short-circuits: if x is false it returns x, otherwise y. \"x or y\"\n\
short-circuits: if x is true it returns x, otherwise y.\n",
    ),
    (
        "break",
        "The \"break\" statement\n\
*********************\n\
\n\
   break_stmt ::= \"break\"\n\
\n\
\"break\" may only occur syntactically nested in a \"for\" or \"while\" loop,\n\
but not nested in a function or class definition within that loop. It\n\
terminates the nearest enclosing loop, skipping the optional \"else\"\n\
clause if the loop has one.\n",
    ),
    (
        "callable-types",
        "Emulating callable objects\n\
**************************\n\
\n\
   object.__call__(self[, args...])\n\
\n\
Called when an instance is \"called\" as a function. If this method is\n\
defined on a class, \"x(arg1, arg2, ...)\" roughly translates to\n\
\"type(x).__call__(x, arg1, arg2, ...)\".\n",
    ),
    (
        "calls",
        "Calls\n\
*****\n\
\n\
A call invokes a callable object (such as a function, method, class, or\n\
instance implementing \"__call__\") with a possibly empty series of\n\
positional and keyword arguments:\n\
\n\
   call ::= primary \"(\" [argument_list [\",\"] | comprehension] \")\"\n\
\n\
Positional arguments may be unpacked with \"*iterable\", and keyword\n\
arguments may be unpacked with \"**mapping\".\n",
    ),
    (
        "class",
        "Class definitions\n\
*****************\n\
\n\
A class definition defines a class object:\n\
\n\
   classdef ::= [decorators] \"class\" classname [type_params] [inheritance] \":\" suite\n\
\n\
The class suite is executed in a newly created local namespace, and the\n\
resulting class object is bound to \"classname\" in the enclosing scope\n\
(after applying any class decorators from bottom to top).\n",
    ),
    (
        "comparisons",
        "Comparisons\n\
***********\n\
\n\
   comparison    ::= or_expr (comp_operator or_expr)*\n\
   comp_operator ::= \"<\" | \">\" | \"==\" | \">=\" | \"<=\" | \"!=\"\n\
                   | \"is\" [\"not\"] | [\"not\"] \"in\"\n\
\n\
Comparisons can be chained arbitrarily: \"x < y <= z\" is equivalent to\n\
\"x < y and y <= z\", except that \"y\" is evaluated only once. \"is\" and\n\
\"is not\" test object identity; \"in\" and \"not in\" test membership.\n",
    ),
    (
        "compound",
        "Compound statements\n\
*******************\n\
\n\
Compound statements contain (groups of) other statements and affect or\n\
control the execution of those statements. Python's compound statements\n\
are \"if\", \"while\", \"for\", \"try\", \"with\", \"match\", \"def\", \"class\", and\n\
their \"async\" variants (\"async def\", \"async for\", \"async with\").\n",
    ),
    (
        "context-managers",
        "With Statement Context Managers\n\
*******************************\n\
\n\
A context manager defines the runtime context established when executing\n\
a \"with\" statement by implementing two methods:\n\
\n\
- \"object.__enter__(self)\": enters the runtime context and returns an\n\
  object bound to the \"as\" target (if present).\n\
- \"object.__exit__(self, exc_type, exc_val, exc_tb)\": exits the runtime\n\
  context; returning a true value suppresses any exception raised inside\n\
  the \"with\" block.\n",
    ),
    (
        "continue",
        "The \"continue\" statement\n\
************************\n\
\n\
   continue_stmt ::= \"continue\"\n\
\n\
\"continue\" may only occur syntactically nested in a \"for\" or \"while\"\n\
loop, and not nested in a function or class definition within that loop.\n\
It continues immediately with the next cycle of the nearest enclosing\n\
loop, executing any enclosing \"finally\" clauses along the way.\n",
    ),
    (
        "conversions",
        "Arithmetic conversions\n\
**********************\n\
\n\
When a description of an arithmetic operator says \"the numeric arguments\n\
are converted to a common type\", the built-in numeric types follow this\n\
coercion order:\n\
\n\
- If either argument is a complex number, the other is converted to complex.\n\
- Otherwise, if either argument is a floating point number, the other is\n\
  converted to float.\n\
- Otherwise, both must be integers and no conversion is necessary.\n",
    ),
    (
        "customization",
        "Basic customization\n\
*******************\n\
\n\
Core special methods for object lifecycle, formatting, comparison, and\n\
truthiness:\n\
\n\
- \"__new__\", \"__init__\", \"__del__\": construction, initialization, finalization.\n\
- \"__repr__\", \"__str__\", \"__bytes__\", \"__format__\": string/bytes conversion.\n\
- \"__lt__\", \"__le__\", \"__eq__\", \"__ne__\", \"__gt__\", \"__ge__\": rich comparisons.\n\
- \"__hash__\": hash value for dict keys and sets.\n\
- \"__bool__\": truth value testing.\n",
    ),
    (
        "debugger",
        "\"pdb\" — The Python Debugger\n\
***************************\n\
\n\
The module \"pdb\" defines an interactive source code debugger for Python\n\
programs. It supports setting (conditional) breakpoints, single stepping\n\
at the source line level, inspection of stack frames, source code\n\
listing, and evaluation of arbitrary Python code in the context of any\n\
stack frame. Call \"breakpoint()\" or \"pdb.set_trace()\" to enter the\n\
debugger.\n",
    ),
    (
        "del",
        "The \"del\" statement\n\
*******************\n\
\n\
   del_stmt ::= \"del\" target_list\n\
\n\
Deletion is recursively defined very similarly to assignment. Deleting a\n\
name removes its binding from the local or global namespace. Deleting an\n\
attribute, subscription, or slicing invokes \"__delattr__\" or \"__delitem__\"\n\
on the target object.\n",
    ),
    (
        "dict",
        "Dictionary displays\n\
*******************\n\
\n\
A dictionary display is a possibly empty series of key/datum pairs or\n\
\"**expression\" unpackings enclosed in curly braces:\n\
\n\
   dict_display ::= \"{\" [key_datum_list | dict_comprehension] \"}\"\n\
\n\
Keys must be hashable. Duplicate keys overwrite earlier entries while\n\
preserving insertion order.\n",
    ),
    (
        "dynamic-features",
        "Interaction with dynamic features\n\
*********************************\n\
\n\
Name resolution of free variables occurs at runtime, not at compile time.\n\
The \"eval()\" and \"exec()\" functions do not have access to the full\n\
environment for resolving names; names may be resolved in the local and\n\
global namespaces of the caller.\n",
    ),
    (
        "else",
        "The \"if\" statement\n\
******************\n\
\n\
The \"else\" clause is used in several compound statements:\n\
\n\
- In \"if\" statements, it runs when the condition and all \"elif\" tests\n\
  are false.\n\
- In \"for\" and \"while\" loops, it runs when the loop terminates normally\n\
  without encountering a \"break\" statement.\n\
- In \"try\" statements, it runs when the \"try\" block raises no exception.\n",
    ),
    (
        "exceptions",
        "Exceptions\n\
**********\n\
\n\
Exceptions are a means of breaking out of the normal flow of control of a\n\
code block in order to handle errors or other exceptional conditions.\n\
All built-in exceptions derive from \"BaseException\", and user-defined\n\
exceptions should inherit from \"Exception\". Exceptions are raised with\n\
\"raise\" and handled with \"try\" ... \"except\" / \"except*\" ... \"finally\".\n",
    ),
    (
        "execmodel",
        "Execution model\n\
***************\n\
\n\
A Python program is constructed from code blocks (modules, function\n\
bodies, and class definitions). Each code block is executed in an\n\
execution frame containing administrative information and references to\n\
local, enclosing, global, and built-in namespaces (LEGB resolution).\n",
    ),
    (
        "exprlists",
        "Expression lists\n\
****************\n\
\n\
   expression_list ::= expression (\",\" expression)* [\",\"]\n\
\n\
An expression list containing at least one comma yields a tuple. The\n\
length of the tuple is the number of expressions in the list. A single\n\
trailing comma is required to create a one-element tuple, such as \"(x,)\".\n",
    ),
    (
        "floating",
        "Floating point literals\n\
***********************\n\
\n\
Floating point literals consist of an integer and/or fractional part\n\
separated by a decimal point and/or an exponent part (\"e\" or \"E\").\n\
Underscores may be used to group digits for readability (e.g. \"1_000.5\").\n\
Examples: \"3.14\", \"10.\", \".001\", \"1e100\", \"3.14e-10\".\n",
    ),
    (
        "for",
        "The \"for\" statement\n\
*******************\n\
\n\
The \"for\" statement iterates over the elements of an iterable:\n\
\n\
   for_stmt ::= \"for\" target_list \"in\" starred_list \":\" suite\n\
                [\"else\" \":\" suite]\n\
\n\
Each item produced by the iterator is assigned to \"target_list\" before\n\
executing the suite. When the iterator is exhausted, the optional \"else\"\n\
suite is executed unless the loop was exited via \"break\".\n",
    ),
    (
        "formatstrings",
        "Format String Syntax\n\
********************\n\
\n\
The \"str.format()\" method, \"format()\" built-in, and f-strings share the\n\
Format Specification Mini-Language:\n\
\n\
   format_spec ::= [[fill]align][sign][z][#][0][width][grouping_option][.precision][type]\n\
\n\
Replacement fields are surrounded by curly braces \"{}\". Optional\n\
conversion flags \"!s\" (str), \"!r\" (repr), and \"!a\" (ascii) may precede\n\
the colon and format specifier.\n",
    ),
    (
        "function",
        "Function definitions\n\
********************\n\
\n\
A function definition defines a user-defined function object:\n\
\n\
   funcdef ::= [decorators] \"def\" funcname [type_params] \"(\" [parameter_list] \")\" [\"->\" expression] \":\" suite\n\
\n\
Parameters may be positional-only (before \"/\"), positional-or-keyword,\n\
var-positional (\"*args\"), keyword-only (after \"*\"), or var-keyword\n\
(\"**kwargs\"). Default parameter values are evaluated once at definition\n\
time.\n",
    ),
    (
        "global",
        "The \"global\" statement\n\
**********************\n\
\n\
   global_stmt ::= \"global\" identifier (\",\" identifier)*\n\
\n\
The \"global\" statement is a declaration which holds for the entire\n\
current code block. It means that the listed identifiers are to be\n\
interpreted as globals (module-level bindings) rather than locals.\n",
    ),
    (
        "id-classes",
        "Reserved classes of identifiers\n\
*******************************\n\
\n\
Certain classes of identifiers have special meanings:\n\
\n\
- \"_*\": not imported by \"from module import *\".\n\
- \"__*_\": system-defined \"dunder\" names (e.g. \"__init__\", \"__name__\").\n\
- \"__*\": class-private names, mangled to \"_ClassName__name\" inside\n\
  class definitions.\n",
    ),
    (
        "identifiers",
        "Identifiers and keywords\n\
************************\n\
\n\
Identifiers (also referred to as names) start with a Unicode letter or\n\
underscore \"_\" followed by zero or more letters, digits, or underscores.\n\
Identifiers are case-sensitive and unlimited in length.\n\
\n\
Keywords (such as \"False\", \"None\", \"True\", \"and\", \"def\", \"class\", \"if\",\n\
\"return\", \"while\", \"with\", \"yield\") are reserved and cannot be used as\n\
ordinary identifiers.\n",
    ),
    (
        "if",
        "The \"if\" statement\n\
******************\n\
\n\
The \"if\" statement is used for conditional execution:\n\
\n\
   if_stmt ::= \"if\" assignment_expression \":\" suite\n\
               (\"elif\" assignment_expression \":\" suite)*\n\
               [\"else\" \":\" suite]\n\
\n\
It selects at most one of the suites by evaluating the expressions one by\n\
one until one is found to be true; then that suite is executed.\n",
    ),
    (
        "imaginary",
        "Imaginary literals\n\
******************\n\
\n\
Imaginary literals are written as an integer or floating point literal\n\
followed by \"j\" or \"J\" (for example \"3.14j\", \"10.j\", \"10j\", \"1e100j\").\n\
An imaginary literal yields a complex number with a real part of 0.0.\n",
    ),
    (
        "import",
        "The \"import\" statement\n\
**********************\n\
\n\
   import_stmt     ::= \"import\" module [\"as\" identifier] (\",\" module [\"as\" identifier])*\n\
                     | \"from\" relative_module \"import\" identifier [\"as\" identifier] (...)\n\
                     | \"from\" relative_module \"import\" \"*\"\n\
\n\
The basic import statement searches for the named module (caching loaded\n\
modules in \"sys.modules\") and binds the result in the local namespace.\n",
    ),
    (
        "in",
        "Membership test operations\n\
**************************\n\
\n\
The operators \"in\" and \"not in\" test for membership. \"x in s\" evaluates\n\
to True if x is a member of s, and False otherwise; \"x not in s\" returns\n\
the negation of \"x in s\".\n\
\n\
For user-defined classes, membership uses \"__contains__\", falling back to\n\
iteration via \"__iter__\" and then sequence indexing via \"__getitem__\".\n",
    ),
    (
        "integers",
        "Integer literals\n\
****************\n\
\n\
Integer literals support decimal (\"42\"), binary (\"0b101010\"), octal\n\
(\"0o52\"), and hexadecimal (\"0x2a\") forms, with optional underscores as\n\
visual separators (\"1_000_000\"). Python integers have arbitrary precision.\n",
    ),
    (
        "lambda",
        "Lambdas\n\
*******\n\
\n\
   lambda_expr ::= \"lambda\" [parameter_list] \":\" expression\n\
\n\
Lambda expressions (lambda forms) create anonymous function objects.\n\
\"lambda parameters: expression\" yields a function object behaving like a\n\
single-expression \"def\" returning the value of \"expression\".\n",
    ),
    (
        "lists",
        "List displays\n\
*************\n\
\n\
A list display is a possibly empty series of expressions or a list\n\
comprehension enclosed in square brackets:\n\
\n\
   list_display ::= \"[\" [starred_list | comprehension] \"]\"\n\
\n\
A list display yields a new mutable list object.\n",
    ),
    (
        "naming",
        "Naming and binding\n\
******************\n\
\n\
Names refer to objects and are introduced by name binding operations\n\
(assignment, function/class definitions, \"import\", \"for\" targets, \"as\"\n\
clauses, and formal parameters).\n\
\n\
Python resolves names lexically using the LEGB rule: Local, Enclosing\n\
function scopes (closures), Global (module), and Built-in scopes. Use\n\
\"global\" or \"nonlocal\" to rebind variables in outer scopes.\n",
    ),
    (
        "nonlocal",
        "The \"nonlocal\" statement\n\
************************\n\
\n\
   nonlocal_stmt ::= \"nonlocal\" identifier (\",\" identifier)*\n\
\n\
The \"nonlocal\" statement causes the listed identifiers to refer to\n\
previously bound variables in the nearest enclosing function scope\n\
excluding globals.\n",
    ),
    (
        "numbers",
        "Numeric literals\n\
****************\n\
\n\
There are three types of numeric literals: integers (\"int\"), floating\n\
point numbers (\"float\"), and imaginary numbers (\"complex\"). Numeric\n\
literals do not include a sign; a leading \"-\" is a separate unary\n\
operator.\n",
    ),
    (
        "numeric-types",
        "Emulating numeric types\n\
***********************\n\
\n\
Classes can emulate numeric operations by implementing special methods:\n\
\n\
- Binary operations: \"__add__\", \"__sub__\", \"__mul__\", \"__matmul__\",\n\
  \"__truediv__\", \"__floordiv__\", \"__mod__\", \"__divmod__\", \"__pow__\",\n\
  \"__lshift__\", \"__rshift__\", \"__and__\", \"__xor__\", \"__or__\".\n\
- Reflected operations: \"__radd__\", \"__rsub__\", etc.\n\
- In-place operations: \"__iadd__\", \"__isub__\", etc.\n\
- Unary & conversion: \"__neg__\", \"__pos__\", \"__abs__\", \"__invert__\",\n\
  \"__complex__\", \"__int__\", \"__float__\", \"__index__\", \"__round__\".\n",
    ),
    (
        "objects",
        "Objects, values and types\n\
*************************\n\
\n\
Objects are Python's abstraction for data. Every object has an identity\n\
(\"id(obj)\"), a type (\"type(obj)\"), and a value. An object's identity\n\
never changes once created; the \"is\" operator compares identities.\n\
Objects whose value can change are mutable (e.g. lists, dicts, sets);\n\
objects whose value cannot change are immutable (e.g. ints, floats,\n\
strings, tuples).\n",
    ),
    (
        "operator-summary",
        "Operator precedence\n\
*******************\n\
\n\
From lowest precedence (least binding) to highest precedence (most binding):\n\
\n\
1.  \":=\" (assignment expression)\n\
2.  \"lambda\"\n\
3.  \"if\" – \"else\" (conditional expression)\n\
4.  \"or\"\n\
5.  \"and\"\n\
6.  \"not x\"\n\
7.  \"in\", \"not in\", \"is\", \"is not\", \"<\", \"<=\", \">\", \">=\", \"!=\", \"==\"\n\
8.  \"|\"\n\
9.  \"^\"\n\
10. \"&\"\n\
11. \"<<\", \">>\"\n\
12. \"+\", \"-\"\n\
13. \"*\", \"@\", \"/\", \"//\", \"%\"\n\
14. \"+x\", \"-x\", \"~x\"\n\
15. \"**\" (right-associative)\n\
16. \"await x\"\n\
17. \"x[index]\", \"x[index:index]\", \"x(arguments...)\", \"x.attribute\"\n\
18. \"(expressions...)\", \"[expressions...]\", \"{key: value...}\", \"{expressions...}\"\n",
    ),
    (
        "pass",
        "The \"pass\" statement\n\
********************\n\
\n\
   pass_stmt ::= \"pass\"\n\
\n\
\"pass\" is a null operation — when it is executed, nothing happens. It is\n\
useful as a placeholder when a statement is required syntactically, but\n\
no code needs to be executed.\n",
    ),
    (
        "power",
        "The power operator\n\
******************\n\
\n\
   power ::= await_primary [\"**\" u_expr]\n\
\n\
The power operator binds more tightly than unary operators on its left\n\
and less tightly than unary operators on its right: \"-2 ** 2\" evaluates\n\
to \"-4\", while \"2 ** -1\" evaluates to \"0.5\".\n",
    ),
    (
        "raise",
        "The \"raise\" statement\n\
*********************\n\
\n\
   raise_stmt ::= \"raise\" [expression [\"from\" expression]]\n\
\n\
Raises an exception instance or class (derived from \"BaseException\").\n\
With no expressions, \"raise\" re-raises the active exception. The \"from\"\n\
clause sets explicit exception chaining via \"__cause__\".\n",
    ),
    (
        "return",
        "The \"return\" statement\n\
**********************\n\
\n\
   return_stmt ::= \"return\" [expression_list]\n\
\n\
\"return\" may only occur syntactically nested in a function definition.\n\
It leaves the current function call with the expression list (or \"None\")\n\
as return value, executing any enclosing \"finally\" clauses first.\n",
    ),
    (
        "sequence-types",
        "Emulating container types\n\
*************************\n\
\n\
Classes can emulate sequences and mappings by implementing:\n\
\n\
- \"__len__(self)\": length of the container.\n\
- \"__getitem__(self, key)\": element/slice lookup (\"self[key]\").\n\
- \"__setitem__(self, key, value)\": element/slice assignment.\n\
- \"__delitem__(self, key)\": element/slice deletion.\n\
- \"__missing__(self, key)\": fallback for dict subclasses.\n\
- \"__iter__(self)\", \"__reversed__(self)\", \"__contains__(self, item)\".\n",
    ),
    (
        "shifting",
        "Shifting operations\n\
*******************\n\
\n\
   shift_expr ::= a_expr | shift_expr (\"<<\" | \">>\") a_expr\n\
\n\
These operators accept integers as arguments. Left shift by n bits is\n\
equivalent to multiplying by \"pow(2, n)\"; right shift by n bits is\n\
equivalent to floor division by \"pow(2, n)\". Negative shift counts raise\n\
ValueError.\n",
    ),
    (
        "slicings",
        "Slicings\n\
********\n\
\n\
A slicing selects a range of items in a sequence object (such as a str,\n\
bytes, tuple, or list):\n\
\n\
   slicing     ::= primary \"[\" slice_list \"]\"\n\
   proper_slice ::= [lower_bound] \":\" [upper_bound] [\":\" [stride]]\n\
\n\
Slicing passes a \"slice(start, stop, step)\" object to \"__getitem__\",\n\
\"__setitem__\", or \"__delitem__\".\n",
    ),
    (
        "specialattrs",
        "Special Attributes\n\
******************\n\
\n\
Common special attributes across Python objects:\n\
\n\
- \"object.__dict__\": mapping storing an object's writable attributes.\n\
- \"instance.__class__\": the class to which a class instance belongs.\n\
- \"class.__bases__\", \"class.__mro__\", \"class.__subclasses__()\".\n\
- \"definition.__name__\", \"definition.__qualname__\", \"definition.__doc__\",\n\
  \"definition.__module__\", \"definition.__annotations__\".\n",
    ),
    (
        "specialnames",
        "Special method names\n\
********************\n\
\n\
A class can implement operations invoked by special syntax (such as\n\
arithmetic, indexing, attribute access, iteration, context management,\n\
and comparisons) by defining methods with double-underscore names\n\
(\"__init__\", \"__repr__\", \"__len__\", \"__getitem__\", \"__iter__\", etc.).\n",
    ),
    (
        "string-methods",
        "String Methods\n\
**************\n\
\n\
Strings implement all common sequence operations along with text\n\
processing methods including: \"capitalize\", \"casefold\", \"center\",\n\
\"count\", \"encode\", \"endswith\", \"expandtabs\", \"find\", \"format\", \"index\",\n\
\"isalnum\", \"isalpha\", \"isascii\", \"isdecimal\", \"isdigit\", \"isidentifier\",\n\
\"islower\", \"isnumeric\", \"isprintable\", \"isspace\", \"istitle\", \"isupper\",\n\
\"join\", \"ljust\", \"lower\", \"lstrip\", \"maketrans\", \"partition\",\n\
\"removeprefix\", \"removesuffix\", \"replace\", \"rfind\", \"rindex\", \"rjust\",\n\
\"rpartition\", \"rsplit\", \"rstrip\", \"split\", \"splitlines\", \"startswith\",\n\
\"strip\", \"swapcase\", \"title\", \"translate\", \"upper\", and \"zfill\".\n",
    ),
    (
        "strings",
        "String and Bytes literals\n\
*************************\n\
\n\
String literals may be enclosed in single (\"'...'\" ), double (\"\\\"...\\\"\"),\n\
or triple quotes (\"'''...'''\", \"\\\"\\\"\\\"...\\\"\\\"\\\"\"). Optional prefixes:\n\
\n\
- \"r\" / \"R\": raw string (backslashes treated literally).\n\
- \"f\" / \"F\": formatted string literal (f-string, PEP 498 / PEP 701).\n\
- \"b\" / \"B\": bytes literal producing a \"bytes\" instance.\n\
- \"u\" / \"U\": Unicode string literal (equivalent to an unprefixed string).\n",
    ),
    (
        "subscriptions",
        "Subscriptions\n\
*************\n\
\n\
A subscription selects an item of a sequence or mapping object:\n\
\n\
   subscription ::= primary \"[\" expression_list \"]\"\n\
\n\
Evaluating \"obj[key]\" calls \"type(obj).__getitem__(obj, key)\" (or\n\
\"__class_getitem__\" on a class object for generic type parameterization).\n",
    ),
    (
        "truth",
        "Truth Value Testing\n\
*******************\n\
\n\
Any object can be tested for truth value in \"if\"/\"while\" conditions or\n\
boolean operations. By default, an object is considered true unless its\n\
class defines \"__bool__()\" returning False or \"__len__()\" returning 0.\n\
Built-in false values include: \"None\", \"False\", zero of any numeric type\n\
(\"0\", \"0.0\", \"0j\"), and empty sequences/collections (\"''\", \"()\", \"[]\",\n\
\"{}\", \"set()\", \"range(0)\").\n",
    ),
    (
        "try",
        "The \"try\" statement\n\
*******************\n\
\n\
The \"try\" statement specifies exception handlers and/or cleanup code:\n\
\n\
   try_stmt ::= \"try\" \":\" suite\n\
                (\"except\" [expression [\"as\" identifier]] \":\" suite)+\n\
                [\"else\" \":\" suite]\n\
                [\"finally\" \":\" suite]\n\
\n\
Python 3.11+ also supports \"except*\" clauses to handle \"ExceptionGroup\"\n\
instances. A \"finally\" suite always executes when leaving the \"try\" block.\n",
    ),
    (
        "types",
        "The standard type hierarchy\n\
***************************\n\
\n\
Built-in core types in Python 3.12 include:\n\
\n\
- None (\"NoneType\"), NotImplemented, Ellipsis\n\
- Numbers: \"int\" (and \"bool\"), \"float\", \"complex\"\n\
- Immutable sequences: \"str\", \"tuple\", \"bytes\", \"range\"\n\
- Mutable sequences: \"list\", \"bytearray\"\n\
- Set types: \"set\", \"frozenset\"\n\
- Mappings: \"dict\"\n\
- Callable types: functions, methods, generators, coroutines, built-ins, classes\n\
- Modules, custom classes, class instances, code/frame/traceback/slice objects\n",
    ),
    (
        "typesfunctions",
        "Functions\n\
*********\n\
\n\
Function objects are created by \"def\" statements and \"lambda\"\n\
expressions. Key attributes include \"__doc__\", \"__name__\", \"__qualname__\",\n\
\"__module__\", \"__defaults__\", \"__kwdefaults__\", \"__code__\",\n\
\"__globals__\", \"__dict__\", \"__closure__\", \"__annotations__\", and\n\
\"__type_params__\".\n",
    ),
    (
        "typesmapping",
        "Mapping Types — \"dict\"\n\
**********************\n\
\n\
A mapping object maps hashable keys to arbitrary values. \"dict\" is\n\
Python's standard mutable mapping type, preserving key insertion order.\n\
Common operations include \"d[key]\", \"key in d\", \"len(d)\", \"d.get()\",\n\
\"d.keys()\", \"d.values()\", \"d.items()\", \"d.pop()\", \"d.setdefault()\",\n\
\"d.update()\", and dictionary union \"d1 | d2\".\n",
    ),
    (
        "typesmethods",
        "Methods\n\
*******\n\
\n\
Methods are functions bound to an instance (or class, for classmethods).\n\
Accessing a function attribute on an instance creates a bound method\n\
object with \"__self__\" (the bound instance) and \"__func__\" (the\n\
underlying function object).\n",
    ),
    (
        "typesmodules",
        "Modules\n\
*******\n\
\n\
Modules are a basic organizational unit of Python code, created by the\n\
import system. Module objects have a namespace implemented by a\n\
dictionary (\"__dict__\") and special attributes including \"__name__\",\n\
\"__doc__\", \"__package__\", \"__loader__\", \"__spec__\", and optionally\n\
\"__file__\" and \"__path__\" (for packages).\n",
    ),
    (
        "typesseq",
        "Sequence Types — \"list\", \"tuple\", \"range\"\n\
*****************************************\n\
\n\
Common operations supported by all sequences: membership (\"x in s\"),\n\
concatenation (\"s + t\"), repetition (\"s * n\"), indexing (\"s[i]\"),\n\
slicing (\"s[i:j:k]\"), \"len(s)\", \"min(s)\", \"max(s)\", \"s.index(x)\",\n\
and \"s.count(x)\".\n",
    ),
    (
        "typesseq-mutable",
        "Mutable Sequence Types\n\
**********************\n\
\n\
Mutable sequences (such as \"list\" and \"bytearray\") additionally support\n\
in-place mutations: item/slice assignment (\"s[i] = x\", \"s[i:j] = t\"),\n\
deletion (\"del s[i:j]\"), \"append()\", \"clear()\", \"copy()\", \"extend()\",\n\
\"insert()\", \"pop()\", \"remove()\", and \"reverse()\" (plus \"sort()\" on lists).\n",
    ),
    (
        "unary",
        "Unary arithmetic and bitwise operations\n\
***************************************\n\
\n\
   u_expr ::= power | \"-\" u_expr | \"+\" u_expr | \"~\" u_expr\n\
\n\
\"-\" yields the numeric negation (\"__neg__\"); \"+\" yields its numeric\n\
argument unchanged (\"__pos__\"); \"~\" yields the bitwise inversion of its\n\
integer argument (\"~x == -(x + 1)\", \"__invert__\").\n",
    ),
    (
        "while",
        "The \"while\" statement\n\
*********************\n\
\n\
The \"while\" statement is used for repeated execution as long as an\n\
expression is true:\n\
\n\
   while_stmt ::= \"while\" assignment_expression \":\" suite\n\
                  [\"else\" \":\" suite]\n\
\n\
If the expression becomes false, the optional \"else\" suite is executed\n\
unless the loop was terminated by \"break\".\n",
    ),
    (
        "with",
        "The \"with\" statement\n\
********************\n\
\n\
The \"with\" statement wraps the execution of a block with methods defined\n\
by a context manager:\n\
\n\
   with_stmt ::= \"with\" with_item (\",\" with_item)* \":\" suite\n\
   with_item ::= expression [\"as\" target]\n\
\n\
Each context manager's \"__enter__()\" is invoked upon entry and its\n\
\"__exit__()\" is guaranteed to run on normal or exceptional exit.\n",
    ),
    (
        "yield",
        "The \"yield\" statement\n\
*********************\n\
\n\
   yield_stmt ::= yield_expression\n\
   yield_expression ::= \"yield\" [expression_list | \"from\" expression]\n\
\n\
Using \"yield\" in a function body makes it a generator function (or async\n\
generator function inside \"async def\"). \"yield\" suspends execution and\n\
produces a value; \"yield from\" delegates to a sub-iterator.\n",
    ),
];

fn build_topics_dict() -> Value {
    let mut d = PyDict::default();
    for &(key, text) in TOPICS_DATA {
        d.insert(PyKey::str_from(key), Value::string(text));
    }
    Value::dict(d)
}

pyrust_module! {
    constants {
        "__package__" => Value::string("pydoc_data"),
        "topics" => build_topics_dict(),
    }
}
