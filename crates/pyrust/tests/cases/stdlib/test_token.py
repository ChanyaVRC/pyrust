import token

# Verify __all__ and constant values match CPython 3.12
print("has_core_all:", all(n in token.__all__ for n in ("tok_name", "ISTERMINAL", "ISNONTERMINAL", "ISEOF", "ENDMARKER", "NAME", "FSTRING_START", "FSTRING_MIDDLE", "FSTRING_END", "ENCODING", "N_TOKENS", "NT_OFFSET")))
print("ENDMARKER:", token.ENDMARKER)
print("NAME:", token.NAME)
print("NUMBER:", token.NUMBER)
print("STRING:", token.STRING)
print("NEWLINE:", token.NEWLINE)
print("INDENT:", token.INDENT)
print("DEDENT:", token.DEDENT)
print("OP:", token.OP)
print("AWAIT:", token.AWAIT)
print("ASYNC:", token.ASYNC)
print("TYPE_IGNORE:", token.TYPE_IGNORE)
print("TYPE_COMMENT:", token.TYPE_COMMENT)
print("SOFT_KEYWORD:", token.SOFT_KEYWORD)
print("FSTRING_START:", token.FSTRING_START)
print("FSTRING_MIDDLE:", token.FSTRING_MIDDLE)
print("FSTRING_END:", token.FSTRING_END)
print("COMMENT:", token.COMMENT)
print("NL:", token.NL)
print("ERRORTOKEN:", token.ERRORTOKEN)
print("ENCODING:", token.ENCODING)
print("N_TOKENS:", token.N_TOKENS)
print("NT_OFFSET:", token.NT_OFFSET)

# Verify every operator constant in EXACT_TOKEN_TYPES
print("exact_count:", len(token.EXACT_TOKEN_TYPES))
for op in sorted(token.EXACT_TOKEN_TYPES):
    code = token.EXACT_TOKEN_TYPES[op]
    print(f"exact:{op}->{code}:{token.tok_name[code]}")

# Verify tok_name covers all standard token numbers
for i in range(token.N_TOKENS):
    assert i in token.tok_name, f"missing {i}"
print("tok_name_count:", len(token.tok_name))
print("tok_name_nt:", token.tok_name[token.NT_OFFSET])

# Predicates
print("ISEOF:", token.ISEOF(token.ENDMARKER), token.ISEOF(token.NAME))
print("ISTERMINAL:", token.ISTERMINAL(token.NAME), token.ISTERMINAL(token.NT_OFFSET))
print("ISNONTERMINAL:", token.ISNONTERMINAL(token.NAME), token.ISNONTERMINAL(token.NT_OFFSET))

# --- Exact namespace / ordering parity (#3348) ---
print("doc:", repr(token.__doc__))
print("all_len:", len(token.__all__))
print("all_head:", token.__all__[:8])
print("all_tail:", token.__all__[-4:])
public = [k for k in vars(token) if not k.startswith("__")]
print("public_order_head:", public[:6])
print("public_order_tail:", public[-8:])
print("public_len:", len(public))
print("tok_name_items_head:", list(token.tok_name.items())[:3])
print("tok_name_items_tail:", list(token.tok_name.items())[-3:])
print("exact_keys_head:", list(token.EXACT_TOKEN_TYPES)[:6])
print("exact_keys_tail:", list(token.EXACT_TOKEN_TYPES)[-4:])
print("tok_name_roundtrip:", all(getattr(token, n) == v for v, n in token.tok_name.items()))
print("exact_values_in_range:", all(0 <= v < token.N_TOKENS for v in token.EXACT_TOKEN_TYPES.values()))
print("all_names_exist:", all(hasattr(token, n) for n in token.__all__))
print("star_import:", end=" ")
ns = {}
exec("from token import *", ns)
print(sorted(k for k in ns if k != "__builtins__") == sorted(token.__all__))

# Predicates: result types and duck-typed arguments
for x in (0, 1, 255, 256, 257, -1, True, False, 255.5, 256.0):
    print("pred:", repr(x), token.ISTERMINAL(x), token.ISNONTERMINAL(x), token.ISEOF(x))
for bad in ("a", None, [1]):
    for fn in (token.ISTERMINAL, token.ISNONTERMINAL):
        try:
            fn(bad)
        except TypeError as e:
            print("pred_error:", type(e).__name__)
print("iseof_str:", token.ISEOF("a"), token.ISEOF(None))

# Mutability of the public tables is per-module-instance, like CPython
print("tok_name_is_dict:", type(token.tok_name) is dict, type(token.EXACT_TOKEN_TYPES) is dict)
print("constants_are_int:", all(type(getattr(token, n)) is int for n in token.tok_name.values()))
