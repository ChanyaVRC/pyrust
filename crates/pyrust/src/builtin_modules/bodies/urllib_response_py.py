"""Response classes used by urllib.

The base class, addbase, defines a minimal file-like interface,
including read() and readline().  The typical response object is an
addinfourl instance, which defines an info() method that returns
headers and a geturl() method that returns the url.
"""

__all__ = ['addbase', 'addclosehook', 'addinfo', 'addinfourl']


class addbase:
    """Base class for addinfo and addclosehook."""

    def __init__(self, fp):
        self.file = fp
        self.name = '<urllib response>'
        self.delete = False
        self.fp = fp
        self._closed = False

    def __repr__(self):
        return '<%s at %r whose fp = %r>' % (
            self.__class__.__name__,
            id(self),
            self.file,
        )

    def __getattr__(self, name):
        file = object.__getattribute__(self, 'file')
        return getattr(file, name)

    @property
    def closed(self):
        file = self.file
        if file is not None and hasattr(file, 'closed'):
            return file.closed
        return self._closed

    def read(self, *args, **kwargs):
        return self.file.read(*args, **kwargs)

    def readline(self, *args, **kwargs):
        return self.file.readline(*args, **kwargs)

    def readlines(self, *args, **kwargs):
        return self.file.readlines(*args, **kwargs)

    def writelines(self, lines):
        for line in lines:
            self.file.write(line)

    def flush(self):
        return self.file.flush()

    def fileno(self):
        return self.file.fileno()

    def readable(self):
        return self.file.readable()

    def writable(self):
        return self.file.writable()

    def seekable(self):
        return self.file.seekable()

    def close(self):
        if not self._closed:
            self._closed = True
            if self.file is not None:
                self.file.close()

    def __iter__(self):
        for line in self.file:
            yield line

    def __next__(self):
        return next(self.file)

    def __enter__(self):
        if self.fp.closed:
            raise ValueError("I/O operation on closed file")
        return self

    def __exit__(self, type, value, traceback):
        self.close()


class addclosehook(addbase):
    """Class to add a close hook to an open file."""

    def __init__(self, fp, closehook, *hookargs):
        super(addclosehook, self).__init__(fp)
        self.closehook = closehook
        self.hookargs = hookargs

    def close(self):
        try:
            closehook = self.closehook
            hookargs = self.hookargs
            if closehook:
                self.closehook = None
                self.hookargs = None
                closehook(*hookargs)
        finally:
            super(addclosehook, self).close()


class addinfo(addbase):
    """class to add an info() method to an open file."""

    def __init__(self, fp, headers):
        super(addinfo, self).__init__(fp)
        self.headers = headers

    def info(self):
        return self.headers


class addinfourl(addinfo):
    """class to add info() and geturl() methods to an open file."""

    def __init__(self, fp, headers, url, code=None):
        super(addinfourl, self).__init__(fp, headers)
        self.url = url
        self.code = code

    @property
    def status(self):
        return self.code

    def getcode(self):
        return self.code

    def geturl(self):
        return self.url
