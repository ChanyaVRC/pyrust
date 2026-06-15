for expr, label in [
    ("[].nonexistent()", "list"),
    ("(1,).nonexistent()", "tuple"),
    ('"hi".nonexistent()', "str"),
    ("{1}.nonexistent()", "set"),
]:
    try:
        eval(expr)
    except AttributeError as e:
        print(f"AttributeError: {e}")
    except Exception as e:
        print(f"WRONG {type(e).__name__}: {e}")
