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
