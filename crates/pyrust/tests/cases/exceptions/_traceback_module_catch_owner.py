try:
    raise RuntimeError("caught in imported module")
except RuntimeError as caught:
    saved = caught
