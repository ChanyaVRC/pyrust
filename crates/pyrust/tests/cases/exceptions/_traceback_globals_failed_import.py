MARKER = "failed-import-root"


def explode():
    raise RuntimeError("failed import")


# Leave the partially initialized module only through the exception traceback.
# Import machinery must remove this module from sys.modules before the caller
# gets control.
explode()
