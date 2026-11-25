"""Low-level networking interface backed by Rust std::net."""

import io
import os
import sys
from enum import IntEnum, IntFlag

_C = _constants_map()


class AddressFamily(IntEnum):
    AF_UNSPEC = _C["AF_UNSPEC"]
    AF_INET = _C["AF_INET"]
    AF_INET6 = _C["AF_INET6"]


class SocketKind(IntEnum):
    SOCK_STREAM = _C["SOCK_STREAM"]
    SOCK_DGRAM = _C["SOCK_DGRAM"]


class AddressInfo(IntFlag):
    AI_PASSIVE = _C["AI_PASSIVE"]
    AI_CANONNAME = _C["AI_CANONNAME"]
    AI_NUMERICHOST = _C["AI_NUMERICHOST"]
    AI_NUMERICSERV = _C["AI_NUMERICSERV"]


AF_UNSPEC = AddressFamily.AF_UNSPEC
AF_INET = AddressFamily.AF_INET
AF_INET6 = AddressFamily.AF_INET6

SOCK_STREAM = SocketKind.SOCK_STREAM
SOCK_DGRAM = SocketKind.SOCK_DGRAM

AI_PASSIVE = AddressInfo.AI_PASSIVE
AI_CANONNAME = AddressInfo.AI_CANONNAME
AI_NUMERICHOST = AddressInfo.AI_NUMERICHOST
AI_NUMERICSERV = AddressInfo.AI_NUMERICSERV

SOL_SOCKET = _C["SOL_SOCKET"]
SO_REUSEADDR = _C["SO_REUSEADDR"]
SO_KEEPALIVE = _C["SO_KEEPALIVE"]
SO_BROADCAST = _C["SO_BROADCAST"]
SO_RCVBUF = _C["SO_RCVBUF"]
SO_SNDBUF = _C["SO_SNDBUF"]
SO_ERROR = _C["SO_ERROR"]

IPPROTO_IP = _C["IPPROTO_IP"]
IPPROTO_TCP = _C["IPPROTO_TCP"]
IPPROTO_UDP = _C["IPPROTO_UDP"]
IPPROTO_IPV6 = _C["IPPROTO_IPV6"]
TCP_NODELAY = _C["TCP_NODELAY"]
IPV6_V6ONLY = _C["IPV6_V6ONLY"]

SHUT_RD = _C["SHUT_RD"]
SHUT_WR = _C["SHUT_WR"]
SHUT_RDWR = _C["SHUT_RDWR"]
SOMAXCONN = _C["SOMAXCONN"]
MSG_PEEK = _C["MSG_PEEK"]

has_ipv6 = True

error = OSError
timeout = TimeoutError


class gaierror(OSError):
    """Address-related error raised by getaddrinfo() and getnameinfo()."""


class herror(OSError):
    """Address-related error raised by legacy host resolution functions."""


_GLOBAL_DEFAULT_TIMEOUT = object()
_default_timeout = None


def getdefaulttimeout():
    """Return the default timeout in seconds (float) for new socket objects."""
    return _default_timeout


def setdefaulttimeout(timeout):
    """Set the default timeout in seconds (float) for new socket objects."""
    global _default_timeout
    if timeout is None:
        _default_timeout = None
        return
    if isinstance(timeout, bool):
        timeout = float(int(timeout))
    elif isinstance(timeout, int):
        timeout = float(timeout)
    elif not isinstance(timeout, float):
        raise TypeError(f"must be real number, not {type(timeout).__name__}")
    if timeout < 0.0 or timeout != timeout or timeout == float("inf"):
        raise ValueError("Timeout value out of range")
    _default_timeout = timeout


def _intenum_converter(value, enum_klass):
    try:
        return enum_klass(value)
    except ValueError:
        return value


class SocketIO(io.RawIOBase):
    """Raw I/O implementation for stream sockets."""

    def __init__(self, sock, mode):
        if mode not in ("r", "w", "rw", "rb", "wb", "rwb"):
            raise ValueError(f"invalid mode: {mode!r}")
        self._sock = sock
        if "b" not in mode:
            mode += "b"
        self._mode = mode
        self._reading = "r" in mode
        self._writing = "w" in mode
        self._closed = False
        self._timeout_occurred = False
        sock._io_refs += 1

    @property
    def mode(self):
        return self._mode

    @property
    def name(self):
        return self._sock.fileno()

    @property
    def closed(self):
        return self._closed

    def _checkClosed(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def _checkReadable(self):
        if not self._reading:
            raise io.UnsupportedOperation("File or stream is not readable.")

    def _checkWritable(self):
        if not self._writing:
            raise io.UnsupportedOperation("File or stream is not writable.")

    def readable(self):
        self._checkClosed()
        return self._reading

    def writable(self):
        self._checkClosed()
        return self._writing

    def seekable(self):
        self._checkClosed()
        return False

    def fileno(self):
        self._checkClosed()
        return self._sock.fileno()

    def readinto(self, b):
        self._checkClosed()
        self._checkReadable()
        if self._timeout_occurred:
            raise OSError("cannot read from timed out object")
        while True:
            try:
                return self._sock.recv_into(b)
            except TimeoutError:
                self._timeout_occurred = True
                raise
            except BlockingIOError:
                return None

    def read(self, size=-1):
        self._checkClosed()
        self._checkReadable()
        if size is None or size < 0:
            chunks = []
            while True:
                data = self._sock.recv(8192)
                if not data:
                    break
                chunks.append(data)
            return b"".join(chunks)
        if size == 0:
            return b""
        return self._sock.recv(size)

    def read1(self, size=-1):
        self._checkClosed()
        self._checkReadable()
        if size is None or size < 0:
            size = 8192
        if size == 0:
            return b""
        return self._sock.recv(size)

    def readline(self, size=-1):
        self._checkClosed()
        self._checkReadable()
        limit = -1 if size is None else size
        out = bytearray()
        while limit < 0 or len(out) < limit:
            ch = self._sock.recv(1)
            if not ch:
                break
            out.extend(ch)
            if ch == b"\n":
                break
        return bytes(out)

    def readlines(self, hint=-1):
        self._checkClosed()
        self._checkReadable()
        lines = []
        total = 0
        while True:
            line = self.readline()
            if not line:
                break
            lines.append(line)
            total += len(line)
            if hint is not None and hint > 0 and total >= hint:
                break
        return lines

    def write(self, b):
        self._checkClosed()
        self._checkWritable()
        try:
            return self._sock.send(b)
        except BlockingIOError:
            return None

    def writelines(self, lines):
        self._checkClosed()
        self._checkWritable()
        for line in lines:
            self.write(line)

    def flush(self):
        self._checkClosed()

    def close(self):
        if self._closed:
            return
        self._closed = True
        self._sock._decref_socketios()

    def __iter__(self):
        self._checkClosed()
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def __enter__(self):
        self._checkClosed()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


class _BufferedSocketFile(io.BufferedIOBase):
    """Buffered binary file object backed by a SocketIO stream."""

    def __init__(self, raw, buffer_size=8192, mode="rb"):
        self._raw = raw
        self._buffer_size = max(1, buffer_size)
        self._mode = mode
        self._rbuf = bytearray()
        self._rpos = 0
        self._wbuf = bytearray()
        self._closed = False

    @property
    def raw(self):
        return self._raw

    @property
    def mode(self):
        return self._mode

    @property
    def name(self):
        return self._raw.name

    @property
    def closed(self):
        return self._closed

    def _checkClosed(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def readable(self):
        self._checkClosed()
        return self._raw.readable()

    def writable(self):
        self._checkClosed()
        return self._raw.writable()

    def seekable(self):
        self._checkClosed()
        return False

    def fileno(self):
        self._checkClosed()
        return self._raw.fileno()

    def _consume_rbuf(self, n):
        avail = len(self._rbuf) - self._rpos
        take = min(n, avail)
        out = bytes(self._rbuf[self._rpos : self._rpos + take])
        self._rpos += take
        if self._rpos >= len(self._rbuf):
            self._rbuf.clear()
            self._rpos = 0
        return out

    def _fill_rbuf(self):
        if self._rpos >= len(self._rbuf):
            self._rbuf.clear()
            self._rpos = 0
        chunk = self._raw._sock.recv(self._buffer_size)
        if chunk:
            self._rbuf.extend(chunk)
        return len(chunk)

    def read(self, size=-1):
        self._checkClosed()
        if not self._raw._reading:
            raise io.UnsupportedOperation("File or stream is not readable.")
        if size is None or size < 0:
            chunks = []
            if self._rpos < len(self._rbuf):
                chunks.append(bytes(self._rbuf[self._rpos :]))
                self._rbuf.clear()
                self._rpos = 0
            while True:
                chunk = self._raw._sock.recv(self._buffer_size)
                if not chunk:
                    break
                chunks.append(chunk)
            return b"".join(chunks)
        if size == 0:
            return b""
        chunks = []
        remaining = size
        if self._rpos < len(self._rbuf):
            part = self._consume_rbuf(remaining)
            chunks.append(part)
            remaining -= len(part)
        while remaining > 0:
            chunk = self._raw._sock.recv(max(remaining, self._buffer_size))
            if not chunk:
                break
            if len(chunk) <= remaining:
                chunks.append(chunk)
                remaining -= len(chunk)
            else:
                chunks.append(chunk[:remaining])
                self._rbuf.extend(chunk[remaining:])
                remaining = 0
        return b"".join(chunks)

    def read1(self, size=-1):
        self._checkClosed()
        if not self._raw._reading:
            raise io.UnsupportedOperation("File or stream is not readable.")
        if size == 0:
            return b""
        if self._rpos < len(self._rbuf):
            want = len(self._rbuf) - self._rpos if (size is None or size < 0) else size
            return self._consume_rbuf(want)
        req = self._buffer_size if (size is None or size < 0) else size
        return self._raw._sock.recv(req)

    def readinto(self, b):
        self._checkClosed()
        if not self._raw._reading:
            raise io.UnsupportedOperation("File or stream is not readable.")
        mv = memoryview(b).cast("B")
        data = self.read(len(mv))
        n = len(data)
        mv[:n] = data
        return n

    def readline(self, size=-1):
        self._checkClosed()
        if not self._raw._reading:
            raise io.UnsupportedOperation("File or stream is not readable.")
        limit = -1 if size is None else size
        if limit == 0:
            return b""
        chunks = []
        total = 0
        while True:
            if self._rpos < len(self._rbuf):
                buf_end = len(self._rbuf)
                if limit > 0:
                    max_end = min(buf_end, self._rpos + (limit - total))
                else:
                    max_end = buf_end
                nl = self._rbuf.find(b"\n", self._rpos, max_end)
                if nl != -1:
                    chunks.append(bytes(self._rbuf[self._rpos : nl + 1]))
                    self._rpos = nl + 1
                    if self._rpos >= len(self._rbuf):
                        self._rbuf.clear()
                        self._rpos = 0
                    break
                part = bytes(self._rbuf[self._rpos : max_end])
                chunks.append(part)
                total += len(part)
                self._rpos = max_end
                if self._rpos >= len(self._rbuf):
                    self._rbuf.clear()
                    self._rpos = 0
                if limit > 0 and total >= limit:
                    break
            if self._fill_rbuf() == 0:
                break
        return b"".join(chunks)

    def readlines(self, hint=-1):
        self._checkClosed()
        lines = []
        total = 0
        while True:
            line = self.readline()
            if not line:
                break
            lines.append(line)
            total += len(line)
            if hint is not None and hint > 0 and total >= hint:
                break
        return lines

    def write(self, b):
        self._checkClosed()
        if not self._raw._writing:
            raise io.UnsupportedOperation("File or stream is not writable.")
        if isinstance(b, (bytes, bytearray)):
            raw_bytes = bytes(b)
        else:
            raw_bytes = memoryview(b).tobytes()
        n = len(raw_bytes)
        self._wbuf.extend(raw_bytes)
        if len(self._wbuf) >= self._buffer_size:
            self.flush()
        return n

    def writelines(self, lines):
        self._checkClosed()
        for line in lines:
            self.write(line)

    def flush(self):
        self._checkClosed()
        if self._wbuf:
            data = bytes(self._wbuf)
            self._wbuf.clear()
            self._raw._sock.sendall(data)

    def close(self):
        if self._closed:
            return
        try:
            if self._raw._writing:
                self.flush()
        finally:
            self._closed = True
            self._raw.close()

    def __iter__(self):
        self._checkClosed()
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def __enter__(self):
        self._checkClosed()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


class _TextSocketFile(io.TextIOBase):
    """Text wrapper backed by a buffered binary socket file."""

    def __init__(
        self,
        buffer,
        encoding=None,
        errors=None,
        newline=None,
        line_buffering=False,
        mode="r",
    ):
        self._buffer = buffer
        self._encoding = encoding if encoding is not None else "utf-8"
        self._errors = errors if errors is not None else "strict"
        if newline not in (None, "", "\n", "\r", "\r\n"):
            raise ValueError(f"illegal newline value: {newline!r}")
        self._newline = newline
        self._line_buffering = line_buffering
        self._mode = mode
        self._tbuf = ""
        self._tpos = 0
        self._pending_cr = False
        self._closed = False
        # Validate encoding eagerly.
        "".encode(self._encoding, self._errors)

    @property
    def buffer(self):
        return self._buffer

    @property
    def encoding(self):
        return self._encoding

    @property
    def errors(self):
        return self._errors

    @property
    def line_buffering(self):
        return self._line_buffering

    @property
    def mode(self):
        return self._mode

    @property
    def name(self):
        return self._buffer.name

    @property
    def closed(self):
        return self._closed

    def _checkClosed(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def readable(self):
        self._checkClosed()
        return self._buffer.readable()

    def writable(self):
        self._checkClosed()
        return self._buffer.writable()

    def seekable(self):
        self._checkClosed()
        return False

    def fileno(self):
        self._checkClosed()
        return self._buffer.fileno()

    def _translate_read(self, text):
        if self._newline is not None:
            return text
        out = []
        i = 0
        n = len(text)
        if self._pending_cr:
            self._pending_cr = False
            if n > 0 and text[0] == "\n":
                i = 1
        while i < n:
            ch = text[i]
            if ch == "\r":
                out.append("\n")
                if i + 1 < n and text[i + 1] == "\n":
                    i += 2
                elif i + 1 == n:
                    self._pending_cr = True
                    i += 1
                else:
                    i += 1
            else:
                out.append(ch)
                i += 1
        return "".join(out)

    def _fill_text(self):
        if self._tpos >= len(self._tbuf):
            self._tbuf = ""
            self._tpos = 0
        raw = self._buffer.read1(8192)
        if not raw:
            return False
        decoded = raw.decode(self._encoding, self._errors)
        self._tbuf += self._translate_read(decoded)
        return True

    def read(self, size=-1):
        self._checkClosed()
        if not self._buffer.readable():
            raise io.UnsupportedOperation("not readable")
        if size is None or size < 0:
            chunks = []
            if self._tpos < len(self._tbuf):
                chunks.append(self._tbuf[self._tpos :])
                self._tbuf = ""
                self._tpos = 0
            rest = self._buffer.read(-1)
            if rest:
                chunks.append(self._translate_read(rest.decode(self._encoding, self._errors)))
            return "".join(chunks)
        if size == 0:
            return ""
        while len(self._tbuf) - self._tpos < size:
            if not self._fill_text():
                break
        end = min(len(self._tbuf), self._tpos + size)
        out = self._tbuf[self._tpos : end]
        self._tpos = end
        if self._tpos >= len(self._tbuf):
            self._tbuf = ""
            self._tpos = 0
        return out

    def readline(self, size=-1):
        self._checkClosed()
        if not self._buffer.readable():
            raise io.UnsupportedOperation("not readable")
        limit = -1 if size is None else size
        if limit == 0:
            return ""
        sep = "\n" if self._newline is None else self._newline
        while True:
            avail = self._tbuf[self._tpos :]
            if avail:
                if sep == "":
                    # Universal newline without translation: break at \r\n, \n, or \r
                    idx = -1
                    end_pos = -1
                    for i, ch in enumerate(avail):
                        if limit > 0 and i >= limit:
                            break
                        if ch == "\n":
                            idx = i
                            end_pos = i + 1
                            break
                        if ch == "\r":
                            if i + 1 < len(avail) and avail[i + 1] == "\n":
                                idx = i
                                end_pos = i + 2
                                break
                            if i + 1 < len(avail):
                                idx = i
                                end_pos = i + 1
                                break
                    if idx != -1:
                        out = avail[:end_pos]
                        self._tpos += end_pos
                        if self._tpos >= len(self._tbuf):
                            self._tbuf = ""
                            self._tpos = 0
                        return out
                else:
                    search_slice = avail[:limit] if limit > 0 else avail
                    pos = search_slice.find(sep)
                    if pos != -1:
                        end_pos = pos + len(sep)
                        out = avail[:end_pos]
                        self._tpos += end_pos
                        if self._tpos >= len(self._tbuf):
                            self._tbuf = ""
                            self._tpos = 0
                        return out
                if limit > 0 and len(avail) >= limit:
                    out = avail[:limit]
                    self._tpos += limit
                    if self._tpos >= len(self._tbuf):
                        self._tbuf = ""
                        self._tpos = 0
                    return out
            if not self._fill_text():
                out = self._tbuf[self._tpos :]
                self._tbuf = ""
                self._tpos = 0
                return out

    def readlines(self, hint=-1):
        self._checkClosed()
        lines = []
        total = 0
        while True:
            line = self.readline()
            if not line:
                break
            lines.append(line)
            total += len(line)
            if hint is not None and hint > 0 and total >= hint:
                break
        return lines

    def write(self, s):
        self._checkClosed()
        if not self._buffer.writable():
            raise io.UnsupportedOperation("not writable")
        if not isinstance(s, str):
            raise TypeError(f"write() argument must be str, not {type(s).__name__}")
        n = len(s)
        out = s
        if self._newline is None:
            if os.linesep != "\n":
                out = out.replace("\n", os.linesep)
        elif self._newline not in ("", "\n"):
            out = out.replace("\n", self._newline)
        data = out.encode(self._encoding, self._errors)
        self._buffer.write(data)
        if self._line_buffering and ("\n" in s or "\r" in s):
            self._buffer.flush()
        return n

    def writelines(self, lines):
        self._checkClosed()
        for line in lines:
            self.write(line)

    def flush(self):
        self._checkClosed()
        self._buffer.flush()

    def close(self):
        if self._closed:
            return
        try:
            if self._buffer.writable():
                self.flush()
        finally:
            self._closed = True
            self._buffer.close()

    def __iter__(self):
        self._checkClosed()
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def __enter__(self):
        self._checkClosed()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


class socket:
    """A subclass-friendly Python 3.12 socket backed by Rust std::net."""

    def __init__(
        self,
        family=-1,
        type=-1,
        proto=-1,
        fileno=None,
        _handle=None,
    ):
        if _handle is not None:
            self._handle = _handle
            self._family = int(family)
            self._type = int(type)
            self._proto = int(proto)
            self._io_refs = 0
            self._closed = False
            return

        if fileno is not None:
            raise NotImplementedError("wrapping an existing fileno is not supported")

        if family == -1:
            family = AF_INET
        if type == -1:
            type = SOCK_STREAM
        if proto == -1:
            proto = 0

        self._family = int(family)
        self._type = int(type)
        self._proto = int(proto)
        self._io_refs = 0
        self._closed = False
        self._handle = _sock_new(self._family, self._type, self._proto, _default_timeout)

    @property
    def family(self):
        return _intenum_converter(self._family, AddressFamily)

    @property
    def type(self):
        return _intenum_converter(self._type, SocketKind)

    @property
    def proto(self):
        return self._proto

    def __repr__(self):
        closed = self._closed
        s = (
            f"<{self.__class__.__module__}.{self.__class__.__name__}"
            f"{' [closed]' if closed else ''} fd={self.fileno()},"
            f" family={self.family!r}, type={self.type!r}, proto={self.proto}"
        )
        if not closed:
            try:
                laddr = self.getsockname()
                if laddr:
                    s += f", laddr={laddr}"
            except OSError:
                pass
            try:
                raddr = self.getpeername()
                if raddr:
                    s += f", raddr={raddr}"
            except OSError:
                pass
        s += ">"
        return s

    def __enter__(self):
        return self

    def __exit__(self, *args):
        if not self._closed:
            self.close()

    def _real_close(self):
        _sock_close(self._handle)

    def close(self):
        self._closed = True
        if self._io_refs <= 0:
            self._real_close()

    def _decref_socketios(self):
        if self._io_refs > 0:
            self._io_refs -= 1
        if self._closed and self._io_refs <= 0:
            self._real_close()

    def fileno(self):
        if self._closed and self._io_refs <= 0:
            return -1
        return _sock_fileno(self._handle)

    def bind(self, address):
        return _sock_bind(self._handle, address, gaierror)

    def listen(self, backlog=None):
        if backlog is None:
            backlog = min(SOMAXCONN, 128)
        elif backlog < 0:
            backlog = 0
        return _sock_listen(self._handle, backlog)

    def accept(self):
        conn_handle, addr = _sock_accept(self._handle, _default_timeout)
        conn = socket(
            family=self._family,
            type=self._type,
            proto=self._proto,
            _handle=conn_handle,
        )
        return conn, addr

    def connect(self, address):
        return _sock_connect(self._handle, address, gaierror)

    def connect_ex(self, address):
        return _sock_connect_ex(self._handle, address, gaierror)

    def send(self, data, flags=0):
        return _sock_send(self._handle, data, flags)

    def sendall(self, data, flags=0):
        return _sock_sendall(self._handle, data, flags)

    def recv(self, bufsize, flags=0):
        return _sock_recv(self._handle, bufsize, flags)

    def recv_into(self, buffer, nbytes=0, flags=0):
        return _sock_recv_into(self._handle, buffer, nbytes, flags)

    def sendto(self, data, *args):
        if len(args) == 1:
            flags = 0
            address = args[0]
        elif len(args) == 2:
            flags = args[0]
            address = args[1]
        else:
            raise TypeError(
                f"sendto() takes 2 or 3 arguments ({len(args) + 1} given)"
            )
        return _sock_sendto(self._handle, data, flags, address, gaierror)

    def recvfrom(self, bufsize, flags=0):
        return _sock_recvfrom(self._handle, bufsize, flags)

    def shutdown(self, how):
        return _sock_shutdown(self._handle, how)

    def setsockopt(self, level, optname, value):
        return _sock_setsockopt(self._handle, level, optname, value)

    def getsockopt(self, level, optname):
        return _sock_getsockopt(self._handle, level, optname)

    def settimeout(self, value):
        return _sock_settimeout(self._handle, value)

    def gettimeout(self):
        return _sock_gettimeout(self._handle)

    def setblocking(self, flag):
        if not isinstance(flag, int):
            raise TypeError("setblocking() argument must be bool")
        self.settimeout(None if flag else 0.0)

    def getblocking(self):
        return self.gettimeout() != 0.0

    def getsockname(self):
        return _sock_getsockname(self._handle)

    def getpeername(self):
        return _sock_getpeername(self._handle)

    def makefile(
        self,
        mode="r",
        buffering=None,
        *,
        encoding=None,
        errors=None,
        newline=None,
    ):
        if not set(mode) <= {"r", "w", "b"}:
            raise ValueError(f"invalid mode {mode!r} (only r, w, b allowed)")
        writing = "w" in mode
        reading = "r" in mode or not writing
        if not (reading or writing):
            raise ValueError("must have at least one of read/write mode")
        binary = "b" in mode
        rawmode = ""
        if reading:
            rawmode += "r"
        if writing:
            rawmode += "w"
        raw = SocketIO(self, rawmode)
        if buffering is None:
            buffering = -1
        if buffering < 0:
            buffering = io.DEFAULT_BUFFER_SIZE
        if buffering == 0:
            if not binary:
                raw.close()
                raise ValueError("unbuffered streams must be binary")
            return raw
        line_buffering = False
        if buffering == 1:
            if not binary:
                line_buffering = True
                buffering = io.DEFAULT_BUFFER_SIZE
        buf_mode = rawmode + "b"
        buf = _BufferedSocketFile(raw, buffer_size=buffering, mode=buf_mode)
        if binary:
            return buf
        text = _TextSocketFile(
            buf,
            encoding=encoding,
            errors=errors,
            newline=newline,
            line_buffering=line_buffering,
            mode=mode,
        )
        return text


SocketType = socket


def gethostname():
    """Return the current host name."""
    return _native_gethostname()


def getfqdn(name=""):
    """Get fully qualified domain name from name."""
    name = name.strip()
    if not name or name == "0.0.0.0":
        name = gethostname()
    try:
        infos = getaddrinfo(name, None, 0, 0, 0, AI_CANONNAME)
    except OSError:
        return name
    for _fam, _type, _proto, canonname, _sa in infos:
        if canonname and "." in canonname:
            return canonname
    return name


def gethostbyname(hostname):
    """Return the IPv4 address string for hostname."""
    return _native_gethostbyname(hostname, gaierror)


def getaddrinfo(host, port, family=0, type=0, proto=0, flags=0):
    """Resolve host and port into list of address info 5-tuples."""
    raw_list = _native_getaddrinfo(
        host, port, int(family), int(type), int(proto), int(flags), gaierror
    )
    addrlist = []
    for fam, kind, pr, canonname, sa in raw_list:
        addrlist.append(
            (
                _intenum_converter(fam, AddressFamily),
                _intenum_converter(kind, SocketKind),
                pr,
                canonname,
                sa,
            )
        )
    return addrlist


def create_connection(
    address,
    timeout=_GLOBAL_DEFAULT_TIMEOUT,
    source_address=None,
    *,
    all_errors=False,
):
    """Connect to *address* and return the socket object."""
    host, port = address
    exceptions = []
    for res in getaddrinfo(host, port, 0, SOCK_STREAM):
        af, socktype, proto, canonname, sa = res
        sock = None
        try:
            sock = socket(af, socktype, proto)
            if timeout is not _GLOBAL_DEFAULT_TIMEOUT:
                sock.settimeout(timeout)
            if source_address:
                sock.bind(source_address)
            sock.connect(sa)
            exceptions.clear()
            return sock
        except OSError as exc:
            if not all_errors:
                exceptions.clear()
            exceptions.append(exc)
            if sock is not None:
                sock.close()

    if len(exceptions):
        try:
            if not all_errors:
                raise exceptions[0]
            raise ExceptionGroup("create_connection failed", exceptions)
        finally:
            exceptions.clear()
    else:
        raise OSError("getaddrinfo returns an empty list")


def create_server(
    address,
    *,
    family=AF_INET,
    backlog=None,
    reuse_port=False,
    dualstack_ipv6=False,
):
    """Convenience function which creates a SOCK_STREAM type socket bound to
    *address* (a 2-tuple (host, port)) and returns the socket object.
    """
    if reuse_port:
        raise ValueError("SO_REUSEPORT not supported on this platform")
    if dualstack_ipv6 and family != AF_INET6:
        raise ValueError("dualstack_ipv6 requires AF_INET6 family")

    sock = socket(family, SOCK_STREAM)
    try:
        if os.name not in ("nt", "cygwin"):
            sock.setsockopt(SOL_SOCKET, SO_REUSEADDR, 1)
        if dualstack_ipv6:
            sock.setsockopt(IPPROTO_IPV6, IPV6_V6ONLY, 0)
        try:
            sock.bind(address)
        except OSError as err:
            msg = f"{err.strerror} (while attempting to bind on address {address!r})"
            raise OSError(err.errno, msg) from None
        if backlog is None:
            sock.listen()
        else:
            sock.listen(backlog)
        return sock
    except BaseException:
        sock.close()
        raise


def htons(x):
    return _native_htons(x)


def htonl(x):
    return _native_htonl(x)


def ntohs(x):
    return _native_ntohs(x)


def ntohl(x):
    return _native_ntohl(x)


def inet_aton(ip_string):
    return _native_inet_aton(ip_string)


def inet_ntoa(packed_ip):
    return _native_inet_ntoa(packed_ip)


def inet_pton(address_family, ip_string):
    return _native_inet_pton(int(address_family), ip_string)


def inet_ntop(address_family, packed_ip):
    return _native_inet_ntop(int(address_family), packed_ip)


__all__ = [
    "AF_UNSPEC",
    "AF_INET",
    "AF_INET6",
    "SOCK_STREAM",
    "SOCK_DGRAM",
    "SOL_SOCKET",
    "SO_REUSEADDR",
    "SO_KEEPALIVE",
    "SO_BROADCAST",
    "SO_RCVBUF",
    "SO_SNDBUF",
    "SO_ERROR",
    "IPPROTO_IP",
    "IPPROTO_TCP",
    "IPPROTO_UDP",
    "IPPROTO_IPV6",
    "TCP_NODELAY",
    "IPV6_V6ONLY",
    "SHUT_RD",
    "SHUT_WR",
    "SHUT_RDWR",
    "SOMAXCONN",
    "AI_PASSIVE",
    "AI_CANONNAME",
    "AI_NUMERICHOST",
    "AI_NUMERICSERV",
    "MSG_PEEK",
    "has_ipv6",
    "AddressFamily",
    "SocketKind",
    "AddressInfo",
    "error",
    "timeout",
    "gaierror",
    "herror",
    "SocketType",
    "socket",
    "SocketIO",
    "create_connection",
    "create_server",
    "gethostname",
    "getfqdn",
    "gethostbyname",
    "getaddrinfo",
    "getdefaulttimeout",
    "setdefaulttimeout",
    "htons",
    "htonl",
    "ntohs",
    "ntohl",
    "inet_aton",
    "inet_ntoa",
    "inet_pton",
    "inet_ntop",
]
