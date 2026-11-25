"""A package for parsing, handling, and generating email messages (issue #3211)."""

__all__ = [
    "header",
    "message",
    "message_from_file",
    "message_from_binary_file",
    "message_from_string",
    "message_from_bytes",
    "parser",
    "policy",
    "utils",
]


def message_from_string(s, *args, **kws):
    """Parse a string into a Message object model."""
    from email.parser import Parser

    return Parser(*args, **kws).parsestr(s)


def message_from_bytes(s, *args, **kws):
    """Parse a bytes string into a Message object model."""
    from email.parser import BytesParser

    return BytesParser(*args, **kws).parsebytes(s)


def message_from_file(fp, *args, **kws):
    """Read a text file and parse its contents into a Message object model."""
    from email.parser import Parser

    return Parser(*args, **kws).parse(fp)


def message_from_binary_file(fp, *args, **kws):
    """Read a binary file and parse its contents into a Message object model."""
    from email.parser import BytesParser

    return BytesParser(*args, **kws).parse(fp)
