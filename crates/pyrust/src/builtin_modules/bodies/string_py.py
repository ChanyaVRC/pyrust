# Python-level members of the `string` module: the `Template` class
# ($-substitution) and the `Formatter` class (PEP 3101 programmatic
# formatting).
#
# CPython's own `string.py` implements `Template` on top of `re` and
# `Formatter` on top of the C helpers `_string.formatter_parser` /
# `_string.formatter_field_name_split`.  pyrust ships neither `re` nor
# `_string`, so both parsers are reimplemented here in pure Python with
# manual scanners that reproduce CPython 3.12's observable behaviour:
#
#   * Template identifiers are ``[_a-zA-Z][_a-zA-Z0-9]*`` (ASCII, the
#     ``(?a:[_a-z]...)`` pattern under ``re.IGNORECASE``).  A bare ``$``
#     not followed by ``$``, a valid identifier, or ``{identifier}`` is an
#     "invalid placeholder" and raises ``ValueError`` reporting the
#     1-based line/column of the ``$``.
#   * The Formatter field-string grammar is ``{field_name!conversion:spec}``
#     with ``{{`` / ``}}`` escapes and recursively-formatted specs.

__all__ = [
    'ascii_letters', 'ascii_lowercase', 'ascii_uppercase', 'capwords',
    'digits', 'hexdigits', 'octdigits', 'printable', 'punctuation',
    'whitespace', 'Formatter', 'Template'
]

_sentinel_dict = {}


def capwords(s, sep=None):
    """Capitalize the words in a string, e.g. " aBc  dEf " -> "Abc Def".

    Split the argument into words using str.split, capitalize each word
    using str.capitalize, and join the capitalized words using str.join.
    If the optional second argument ``sep`` is absent or None, runs of
    whitespace characters are replaced by a single space and leading and
    trailing whitespace are removed; otherwise ``sep`` is used to split
    and join the words.
    """
    return (sep or " ").join(map(str.capitalize, s.split(sep)))


def _is_id_start(ch):
    return ch == "_" or ("a" <= ch <= "z") or ("A" <= ch <= "Z")


def _is_id_cont(ch):
    return _is_id_start(ch) or ("0" <= ch <= "9")


class Template:
    """A string class for supporting $-substitutions."""

    delimiter = "$"

    def __init__(self, template):
        self.template = template

    def _invalid(self, dollar):
        # Report the 1-based line/column of the invalid placeholder.  CPython
        # measures from the position *after* the delimiter (its regex
        # ``invalid`` group matches at ``dollar + 1``), and splits on
        # universal newlines via splitlines, not just \n.
        i = dollar + 1
        lines = self.template[:i].splitlines(keepends=True)
        if not lines:
            colno = 1
            lineno = 1
        else:
            colno = i - len("".join(lines[:-1]))
            lineno = len(lines)
        raise ValueError(
            "Invalid placeholder in string: line %d, col %d" % (lineno, colno)
        )

    def _scan(self, mapping, safe):
        # Single left-to-right pass over the template, building the result.
        # Returns the substituted string.  ``safe`` selects safe_substitute
        # semantics (missing keys and bad placeholders pass through verbatim).
        delim = self.delimiter
        template = self.template
        out = []
        i = 0
        n = len(template)
        while i < n:
            ch = template[i]
            if ch != delim:
                # Copy a run of non-delimiter characters.
                start = i
                i += 1
                while i < n and template[i] != delim:
                    i += 1
                out.append(template[start:i])
                continue
            # ch == delim
            dollar = i
            j = i + 1
            if j >= n:
                # Trailing bare delimiter -> invalid placeholder.
                if safe:
                    out.append(template[dollar:])
                    break
                self._invalid(dollar)
            nxt = template[j]
            if nxt == delim:
                # Escaped delimiter "$$".
                out.append(delim)
                i = j + 1
                continue
            if nxt == "{":
                k = j + 1
                if k < n and _is_id_start(template[k]):
                    k += 1
                    while k < n and _is_id_cont(template[k]):
                        k += 1
                    if k < n and template[k] == "}":
                        name = template[j + 1 : k]
                        out.append(
                            self._lookup(mapping, name, template, dollar, k + 1, safe)
                        )
                        i = k + 1
                        continue
                # Malformed brace expression.
                if safe:
                    out.append(delim)
                    i = j
                    continue
                self._invalid(dollar)
            if _is_id_start(nxt):
                k = j + 1
                while k < n and _is_id_cont(template[k]):
                    k += 1
                name = template[j:k]
                out.append(self._lookup(mapping, name, template, dollar, k, safe))
                i = k
                continue
            # Bare delimiter followed by something else -> invalid.
            if safe:
                out.append(delim)
                i = j
                continue
            self._invalid(dollar)
        return "".join(out)

    def _lookup(self, mapping, name, template, dollar, end, safe):
        if not safe:
            return str(mapping[name])
        try:
            return str(mapping[name])
        except KeyError:
            # Pass the original placeholder text through verbatim.
            return template[dollar:end]

    def substitute(self, mapping=_sentinel_dict, **kws):
        if mapping is _sentinel_dict:
            mapping = kws
        elif kws:
            merged = dict(mapping)
            merged.update(kws)
            mapping = merged
        return self._scan(mapping, False)

    def safe_substitute(self, mapping=_sentinel_dict, **kws):
        if mapping is _sentinel_dict:
            mapping = kws
        elif kws:
            merged = dict(mapping)
            merged.update(kws)
            mapping = merged
        return self._scan(mapping, True)

    def get_identifiers(self):
        ids = []
        template = self.template
        delim = self.delimiter
        i = 0
        n = len(template)
        while i < n:
            if template[i] != delim:
                i += 1
                continue
            dollar = i
            j = i + 1
            if j >= n:
                self._invalid(dollar)
            nxt = template[j]
            if nxt == delim:
                i = j + 1
                continue
            name = None
            if nxt == "{":
                k = j + 1
                if k < n and _is_id_start(template[k]):
                    k += 1
                    while k < n and _is_id_cont(template[k]):
                        k += 1
                    if k < n and template[k] == "}":
                        name = template[j + 1 : k]
                        i = k + 1
                if name is None:
                    self._invalid(dollar)
            elif _is_id_start(nxt):
                k = j + 1
                while k < n and _is_id_cont(template[k]):
                    k += 1
                name = template[j:k]
                i = k
            else:
                self._invalid(dollar)
            if name is not None and name not in ids:
                ids.append(name)
        return ids

    def is_valid(self):
        try:
            self.get_identifiers()
        except ValueError:
            return False
        return True


# ----------------------------------------------------------------------
# Formatter (PEP 3101)


def _formatter_parser(format_string):
    # Reimplements _string.formatter_parser: yields
    # (literal_text, field_name, format_spec, conversion) tuples.
    out = []
    s = format_string
    n = len(s)
    i = 0
    literal = []
    while i < n:
        ch = s[i]
        if ch == "{":
            if i + 1 < n and s[i + 1] == "{":
                literal.append("{")
                i += 2
                continue
            # Start of a replacement field.  A '{' with nothing following it
            # (the last char of the string) is CPython's "Single '{'" case;
            # a '{' followed by content but no closing '}' is "expected '}'".
            literal_text = "".join(literal)
            literal = []
            i += 1
            if i >= n:
                raise ValueError("Single '{' encountered in format string")
            # Parse field name up to '!' (conversion), ':' (spec) or '}'.
            field_start = i
            depth = 0
            while i < n:
                c = s[i]
                if c == "{":
                    depth += 1
                elif c == "}":
                    if depth == 0:
                        break
                    depth -= 1
                elif c in "!:" and depth == 0:
                    break
                i += 1
            field_name = s[field_start:i]
            conversion = None
            format_spec = ""
            if i < n and s[i] == "!":
                i += 1
                if i >= n:
                    raise ValueError(
                        "end of string while looking for conversion specifier"
                    )
                conversion = s[i]
                i += 1
            in_spec = False
            if i < n and s[i] == ":":
                in_spec = True
                i += 1
                spec_start = i
                depth = 0
                while i < n:
                    c = s[i]
                    if c == "{":
                        depth += 1
                    elif c == "}":
                        if depth == 0:
                            break
                        depth -= 1
                    i += 1
                format_spec = s[spec_start:i]
            if i >= n or s[i] != "}":
                # A format spec that ran to end-of-string without its closing
                # '}' is CPython's "unmatched '{' in format spec"; the field
                # name reaching EOS without a spec is "expected '}'".
                if in_spec:
                    raise ValueError("unmatched '{' in format spec")
                raise ValueError("expected '}' before end of string")
            i += 1
            out.append((literal_text, field_name, format_spec, conversion))
        elif ch == "}":
            if i + 1 < n and s[i + 1] == "}":
                literal.append("}")
                i += 2
                continue
            raise ValueError("Single '}' encountered in format string")
        else:
            literal.append(ch)
            i += 1
    if literal:
        out.append(("".join(literal), None, None, None))
    return out


def _field_name_split(field_name):
    # Reimplements _string.formatter_field_name_split: returns
    # (first, rest) where first is an int (auto/manual arg index) or str
    # (keyword), and rest yields (is_attr, key) tuples.
    n = len(field_name)
    i = 0
    while i < n and field_name[i] not in ".[":
        i += 1
    first = field_name[:i]
    if first.isdigit():
        first = int(first)

    def rest():
        idx = i
        while idx < n:
            c = field_name[idx]
            if c == ".":
                idx += 1
                start = idx
                while idx < n and field_name[idx] not in ".[":
                    idx += 1
                yield (True, field_name[start:idx])
            elif c == "[":
                idx += 1
                start = idx
                while idx < n and field_name[idx] != "]":
                    idx += 1
                key = field_name[start:idx]
                if idx < n:
                    idx += 1  # consume ']'
                if key.isdigit():
                    key = int(key)
                yield (False, key)
            else:
                raise ValueError(
                    "Only '.' or '[' may follow ']' in format field specifier"
                )

    return first, rest()


class Formatter:
    def format(self, format_string, /, *args, **kwargs):
        return self.vformat(format_string, args, kwargs)

    def vformat(self, format_string, args, kwargs):
        used_args = set()
        result, _ = self._vformat(format_string, args, kwargs, used_args, 2)
        self.check_unused_args(used_args, args, kwargs)
        return result

    def _vformat(
        self, format_string, args, kwargs, used_args, recursion_depth, auto_arg_index=0
    ):
        if recursion_depth < 0:
            raise ValueError("Max string recursion exceeded")
        result = []
        for literal_text, field_name, format_spec, conversion in self.parse(
            format_string
        ):
            if literal_text:
                result.append(literal_text)
            if field_name is not None:
                if field_name == "":
                    if auto_arg_index is False:
                        raise ValueError(
                            "cannot switch from manual field "
                            "specification to automatic field "
                            "numbering"
                        )
                    field_name = str(auto_arg_index)
                    auto_arg_index += 1
                elif field_name.isdigit():
                    if auto_arg_index:
                        raise ValueError(
                            "cannot switch from manual field "
                            "specification to automatic field "
                            "numbering"
                        )
                    auto_arg_index = False
                obj, arg_used = self.get_field(field_name, args, kwargs)
                used_args.add(arg_used)
                obj = self.convert_field(obj, conversion)
                format_spec, auto_arg_index = self._vformat(
                    format_spec,
                    args,
                    kwargs,
                    used_args,
                    recursion_depth - 1,
                    auto_arg_index=auto_arg_index,
                )
                result.append(self.format_field(obj, format_spec))
        return "".join(result), auto_arg_index

    def get_value(self, key, args, kwargs):
        if isinstance(key, int):
            return args[key]
        else:
            return kwargs[key]

    def check_unused_args(self, used_args, args, kwargs):
        pass

    def format_field(self, value, format_spec):
        return format(value, format_spec)

    def convert_field(self, value, conversion):
        if conversion is None:
            return value
        elif conversion == "s":
            return str(value)
        elif conversion == "r":
            return repr(value)
        elif conversion == "a":
            return ascii(value)
        raise ValueError("Unknown conversion specifier {0!s}".format(conversion))

    def parse(self, format_string):
        return _formatter_parser(format_string)

    def get_field(self, field_name, args, kwargs):
        first, rest = _field_name_split(field_name)
        obj = self.get_value(first, args, kwargs)
        for is_attr, i in rest:
            if is_attr:
                obj = getattr(obj, i)
            else:
                obj = obj[i]
        return obj, first
