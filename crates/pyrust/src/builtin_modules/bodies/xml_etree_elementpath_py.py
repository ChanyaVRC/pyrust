"""
XPath subset evaluator for xml.etree.ElementTree.
"""

__package__ = "xml.etree"

_WHITESPACE = frozenset(" \t\r\n")
_OPERATORS = frozenset("/[]()@=*+-")


def _scan_tokens(pattern):
    """Tokenize an ElementPath expression into (op, tag) pairs."""
    n = len(pattern)
    i = 0
    while i < n:
        ch = pattern[i]
        if ch in _WHITESPACE:
            j = i + 1
            while j < n and pattern[j] in _WHITESPACE:
                j += 1
            yield (pattern[i:j], "")
            i = j
            continue
        if ch == "'" or ch == '"':
            end = pattern.find(ch, i + 1)
            if end == -1:
                yield ("", pattern[i:])
                return
            yield (pattern[i:end + 1], "")
            i = end + 1
            continue
        if ch == "/" and i + 1 < n and pattern[i + 1] == "/":
            yield ("//", "")
            i += 2
            continue
        if ch == "." and i + 1 < n and pattern[i + 1] == ".":
            yield ("..", "")
            i += 2
            continue
        if ch == "!" and i + 1 < n and pattern[i + 1] == "=":
            yield ("!=", "")
            i += 2
            continue
        if ch == "." and (i + 1 == n or pattern[i + 1] in "/[]()@=*+-\t\r\n !"):
            yield (".", "")
            i += 1
            continue
        if ch == "{":
            close = pattern.find("}", i + 1)
            if close == -1:
                raise SyntaxError("invalid path")
            j = close + 1
            if j < n and pattern[j] == "*":
                j += 1
            else:
                while j < n and pattern[j] not in _OPERATORS and pattern[j] not in _WHITESPACE and pattern[j] not in "!'\"":
                    j += 1
            yield ("", pattern[i:j])
            i = j
            continue
        if ch in _OPERATORS:
            yield (ch, "")
            i += 1
            continue
        # Tag / identifier / number / prefix:local
        j = i
        while j < n and pattern[j] not in _OPERATORS and pattern[j] not in _WHITESPACE and pattern[j] not in "!'\"{}":
            j += 1
        if j == i:
            # Unexpected single character (e.g. lone '!')
            yield (ch, "")
            i += 1
        else:
            # Handle prefix:* like p:*
            if j < n and pattern[j] == "*" and pattern[j - 1] == ":":
                j += 1
            yield ("", pattern[i:j])
            i = j


def xpath_tokenizer(pattern, namespaces=None):
    default_namespace = namespaces.get("") if namespaces else None
    parsing_attribute = False
    for op, tag in _scan_tokens(pattern):
        if tag:
            if tag[0] == "{":
                yield ("", tag)
            elif ":" in tag:
                if namespaces:
                    prefix, uri = tag.split(":", 1)
                    if prefix not in namespaces:
                        raise SyntaxError("prefix %r not found in prefix map" % prefix)
                    yield ("", "{%s}%s" % (namespaces[prefix], uri))
                else:
                    yield ("", tag)
            elif default_namespace and not parsing_attribute:
                yield ("", "{%s}%s" % (default_namespace, tag))
            else:
                yield ("", tag)
            parsing_attribute = False
        else:
            if op == "@":
                parsing_attribute = True
            elif op.strip():
                parsing_attribute = False
            yield (op, "")


def get_parent_map(context):
    parent_map = context.parent_map
    if parent_map is None:
        context.parent_map = parent_map = {}
        for p in context.root.iter():
            for e in p:
                parent_map[e] = p
    return parent_map


def _is_wildcard_tag(tag):
    return tag == "*" or tag == "{*}*" or (tag.startswith("{") and tag.endswith("}*")) or (tag.startswith("{*}") and len(tag) > 3)


def _match_tag(elem_tag, pattern_tag):
    if not isinstance(elem_tag, str):
        return False
    if pattern_tag == "*" or pattern_tag == "{*}*":
        return True
    if pattern_tag.startswith("{*}"):
        local = pattern_tag[3:]
        if elem_tag.startswith("{"):
            idx = elem_tag.find("}")
            return idx != -1 and elem_tag[idx + 1:] == local
        return elem_tag == local
    if pattern_tag.startswith("{") and pattern_tag.endswith("}*"):
        prefix = pattern_tag[:-1]
        return elem_tag.startswith(prefix)
    return elem_tag == pattern_tag


def _prepare_tag(tag):
    if tag == "{*}*":
        return lambda elem: isinstance(elem.tag, str)
    if tag.startswith("{*}"):
        local = tag[3:]
        def select(elem):
            t = elem.tag
            if not isinstance(t, str):
                return False
            if t.startswith("{"):
                idx = t.find("}")
                return idx != -1 and t[idx + 1:] == local
            return t == local
        return select
    if tag.startswith("{") and tag.endswith("}*"):
        prefix = tag[:-1]
        return lambda elem: isinstance(elem.tag, str) and elem.tag.startswith(prefix)
    return lambda elem: elem.tag == tag


def prepare_child(next_token, token):
    tag = token[1]
    if _is_wildcard_tag(tag):
        matcher = _prepare_tag(tag)
        def select(context, result):
            for elem in result:
                for e in elem:
                    if matcher(e):
                        yield e
        return select
    def select(context, result):
        for elem in result:
            for e in elem:
                if e.tag == tag:
                    yield e
    return select


def prepare_star(next_token, token):
    def select(context, result):
        for elem in result:
            yield from elem
    return select


def prepare_self(next_token, token):
    def select(context, result):
        yield from result
    return select


def prepare_descendant(next_token, token):
    try:
        token = next_token()
    except StopIteration:
        return
    if token[0] == "*":
        tag = "*"
    elif not token[0]:
        tag = token[1]
    else:
        raise SyntaxError("invalid descendant")
    if tag == "*":
        def select(context, result):
            for elem in result:
                for e in elem.iter():
                    if e is not elem:
                        yield e
        return select
    if _is_wildcard_tag(tag):
        matcher = _prepare_tag(tag)
        def select(context, result):
            for elem in result:
                for e in elem.iter():
                    if e is not elem and matcher(e):
                        yield e
        return select
    def select(context, result):
        for elem in result:
            for e in elem.iter(tag):
                if e is not elem:
                    yield e
    return select


def prepare_parent(next_token, token):
    def select(context, result):
        parent_map = get_parent_map(context)
        result_map = {}
        for elem in result:
            if elem in parent_map:
                parent = parent_map[elem]
                if parent not in result_map:
                    result_map[parent] = None
                    yield parent
    return select


def _unquote(s):
    if len(s) >= 2 and s[0] == s[-1] and s[0] in ("'", '"'):
        return s[1:-1]
    return s


def prepare_predicate(next_token, token):
    signature = []
    predicate = []
    while True:
        try:
            token = next_token()
        except StopIteration:
            raise SyntaxError("invalid predicate")
        if token[0] == "]":
            break
        if token[0] and not token[0].strip():
            # Skip whitespace inside predicates
            continue
        if token[0] in ("'", '"') or (token[0] and token[0][0] in ("'", '"')):
            signature.append("'")
            predicate.append(_unquote(token[0]))
        elif token[0]:
            signature.append(token[0])
            predicate.append(token[0])
        else:
            signature.append("")
            predicate.append(token[1])

    sig = "".join(signature)

    # [@attr]
    if sig == "@":
        key = predicate[1]
        def select(context, result):
            for elem in result:
                if elem.get(key) is not None:
                    yield elem
        return select

    # [@attr='value'] or [@attr!='value']
    if sig == "@='" or sig == "@!='":
        key = predicate[1]
        op = predicate[2]
        value = predicate[3]
        if op == "=":
            def select(context, result):
                for elem in result:
                    if elem.get(key) == value:
                        yield elem
        else:
            def select(context, result):
                for elem in result:
                    v = elem.get(key)
                    if v is not None and v != value:
                        yield elem
        return select

    # [.='value'] or [.!='value']
    if sig == ".='" or sig == ".!='":
        op = predicate[1]
        value = predicate[2]
        if op == "=":
            def select(context, result):
                for elem in result:
                    if "".join(elem.itertext()) == value:
                        yield elem
        else:
            def select(context, result):
                for elem in result:
                    if "".join(elem.itertext()) != value:
                        yield elem
        return select

    # [tag] or [a/b] (subpath existence)
    if sig and not any(c in sig for c in "='!()@-") and all(c in "/*.:" or c == "" for c in signature):
        # Reconstruct subpath string from predicate tokens
        subpath = "".join(predicate)
        def select(context, result):
            for elem in result:
                if elem.find(subpath) is not None:
                    yield elem
        return select

    # Single tag existence when signature is "" (empty string from single tag token)
    if signature == [""] and not predicate[0].isdigit():
        tag = predicate[0]
        def select(context, result):
            for elem in result:
                if elem.find(tag) is not None:
                    yield elem
        return select

    # [subpath='value'] or [subpath!='value'] (including [tag='value'] and [a/b='value'])
    if "=" in signature or "!=" in signature:
        eq_idx = signature.index("=") if "=" in signature else signature.index("!=")
        op = signature[eq_idx]
        if eq_idx >= 1 and signature[eq_idx + 1:] == ["'"]:
            lhs_sig = signature[:eq_idx]
            if all(c in ("", "/", "*", ".", "//") for c in lhs_sig):
                subpath = "".join(predicate[:eq_idx])
                value = predicate[eq_idx + 1]
                if op == "=":
                    def select(context, result):
                        for elem in result:
                            for e in elem.findall(subpath):
                                if "".join(e.itertext()) == value:
                                    yield elem
                                    break
                else:
                    def select(context, result):
                        for elem in result:
                            for e in elem.findall(subpath):
                                if "".join(e.itertext()) != value:
                                    yield elem
                                    break
                return select

    # [position] — 1-based integer index, [last()], [last()-n]
    index = None
    if signature == [""] and predicate[0].isdigit():
        index = int(predicate[0]) - 1
        if index < 0:
            raise SyntaxError("XPath position >= 1 expected")
    elif signature == ["", "(", ")"] and predicate[0] == "last":
        index = -1
    elif signature == ["", "(", ")", "-", ""] and predicate[0] == "last" and predicate[4].isdigit():
        index = -1 - int(predicate[4])

    if index is not None:
        def select(context, result):
            parent_map = get_parent_map(context)
            parent_children = {}
            for elem in result:
                if elem in parent_map:
                    parent = parent_map[elem]
                    key = (id(parent), elem.tag)
                    children = parent_children.get(key)
                    if children is None:
                        children = [c for c in parent if c.tag == elem.tag]
                        parent_children[key] = children
                    try:
                        if children[index] is elem:
                            yield elem
                    except IndexError:
                        pass
        return select

    raise SyntaxError("invalid predicate")


ops = {
    "": prepare_child,
    "*": prepare_star,
    ".": prepare_self,
    "..": prepare_parent,
    "//": prepare_descendant,
    "[": prepare_predicate,
}

_cache = {}


class _SelectorContext:
    parent_map = None

    def __init__(self, root):
        self.root = root


def iterfind(elem, path, namespaces=None):
    if path[-1:] == "/":
        path = path + "*"

    cache_key = (path,)
    if namespaces:
        cache_key += tuple(sorted(namespaces.items()))

    selector = _cache.get(cache_key)
    if selector is None:
        if len(_cache) > 100:
            _cache.clear()
        if path[:1] == "/":
            raise SyntaxError("cannot use absolute path on element")
        stream = iter(xpath_tokenizer(path, namespaces))
        next_token = stream.__next__
        try:
            token = next_token()
        except StopIteration:
            return
        selector = []
        while True:
            # Skip stray whitespace between path tokens if any
            if token[0] and not token[0].strip():
                try:
                    token = next_token()
                    continue
                except StopIteration:
                    break
            try:
                prep = ops[token[0]]
            except KeyError:
                raise SyntaxError("invalid path")
            step = prep(next_token, token)
            if step is not None:
                selector.append(step)
            try:
                token = next_token()
                if token[0] == "/":
                    token = next_token()
            except StopIteration:
                break
        _cache[cache_key] = selector

    result = [elem]
    context = _SelectorContext(elem)
    for select in selector:
        result = select(context, result)
    return result


def find(elem, path, namespaces=None):
    return next(iterfind(elem, path, namespaces), None)


def findall(elem, path, namespaces=None):
    return list(iterfind(elem, path, namespaces))


def findtext(elem, path, default=None, namespaces=None):
    elem = find(elem, path, namespaces)
    if elem is None:
        return default
    return elem.text or ""
