"""
Python 3.12 ``array`` module backed by native binary encoding helpers in
``array.rs`` (issue #2809).
"""

typecodes = "bBuhHiIlLqQfd"


def _array_reconstructor(arraytype, typecode, mformat_code, items):
    arr = arraytype(typecode)
    if isinstance(items, (bytes, bytearray)):
        arr.frombytes(items)
    else:
        arr.extend(items)
    return arr


class array:
    __hash__ = None

    def __init__(self, typecode, initializer=None, /):
        itemsize = _typecode_itemsize(typecode)
        self._typecode = typecode
        self._itemsize = itemsize
        self._buf = bytearray()
        if initializer is not None:
            if isinstance(initializer, (bytes, bytearray)):
                self.frombytes(initializer)
            elif isinstance(initializer, str):
                if typecode == "u":
                    self.fromunicode(initializer)
                else:
                    raise TypeError(
                        "cannot use a str to initialize an array with typecode '"
                        + typecode
                        + "'"
                    )
            elif isinstance(initializer, array):
                if initializer._typecode == typecode:
                    self._buf.extend(initializer._buf)
                else:
                    for item in initializer:
                        self.append(item)
            else:
                self.extend(initializer)

    @property
    def typecode(self):
        return self._typecode

    @property
    def itemsize(self):
        return self._itemsize

    def __len__(self):
        return len(self._buf) // self._itemsize

    def __bool__(self):
        return len(self._buf) != 0

    def append(self, v, /):
        self._buf.extend(_encode_item(self._typecode, v))

    def extend(self, bb, /):
        if isinstance(bb, array):
            if bb._typecode != self._typecode:
                raise TypeError("can only extend with array of same kind")
            if bb is self:
                self._buf.extend(bytes(self._buf))
            else:
                self._buf.extend(bb._buf)
            return
        for item in bb:
            self._buf.extend(_encode_item(self._typecode, item))

    def insert(self, i, v, /):
        idx = _as_index(i)
        n = len(self)
        if idx < 0:
            idx += n
            if idx < 0:
                idx = 0
        elif idx > n:
            idx = n
        encoded = _encode_item(self._typecode, v)
        pos = idx * self._itemsize
        self._buf[pos:pos] = encoded

    def pop(self, i=-1, /):
        n = len(self)
        if n == 0:
            raise IndexError("pop from empty array")
        idx = _as_index(i)
        if idx < 0:
            idx += n
        if idx < 0 or idx >= n:
            raise IndexError("pop index out of range")
        val = _decode_item(self._typecode, self._buf, idx)
        pos = idx * self._itemsize
        del self._buf[pos : pos + self._itemsize]
        return val

    def remove(self, v, /):
        n = len(self)
        for idx in range(n):
            if _decode_item(self._typecode, self._buf, idx) == v:
                pos = idx * self._itemsize
                del self._buf[pos : pos + self._itemsize]
                return
        raise ValueError("array.remove(x): x not in array")

    def reverse(self):
        n = len(self)
        sz = self._itemsize
        buf = self._buf
        lo = 0
        hi = n - 1
        while lo < hi:
            a = lo * sz
            b = hi * sz
            tmp = bytes(buf[a : a + sz])
            buf[a : a + sz] = buf[b : b + sz]
            buf[b : b + sz] = tmp
            lo += 1
            hi -= 1

    def index(self, v, start=0, stop=None, /):
        n = len(self)
        s = _as_index(start)
        if s < 0:
            s += n
            if s < 0:
                s = 0
        elif s > n:
            s = n
        if stop is None:
            e = n
        else:
            e = _as_index(stop)
            if e < 0:
                e += n
                if e < 0:
                    e = 0
            elif e > n:
                e = n
        for idx in range(s, e):
            if _decode_item(self._typecode, self._buf, idx) == v:
                return idx
        raise ValueError("array.index(x): x not in array")

    def count(self, v, /):
        c = 0
        for idx in range(len(self)):
            if _decode_item(self._typecode, self._buf, idx) == v:
                c += 1
        return c

    def byteswap(self):
        _byteswap(self._buf, self._itemsize)

    def tobytes(self):
        return bytes(self._buf)

    def frombytes(self, buffer, /):
        if isinstance(buffer, bytearray):
            raw = bytes(buffer)
        elif isinstance(buffer, bytes):
            raw = buffer
        elif isinstance(buffer, memoryview):
            raw = buffer.tobytes()
        elif hasattr(type(buffer), "__buffer__"):
            with memoryview(buffer) as _mv:
                raw = _mv.tobytes()
        else:
            raise TypeError(
                "a bytes-like object is required, not '" + type(buffer).__name__ + "'"
            )
        if len(raw) % self._itemsize != 0:
            raise ValueError("bytes length not a multiple of item size")
        self._buf.extend(raw)

    def __buffer__(self, flags, /):
        return _make_buffer_view(self, self._buf, self._typecode, flags)

    def tolist(self):
        return _decode_all(self._typecode, self._buf)

    def fromlist(self, list_val, /):
        if not isinstance(list_val, list):
            raise TypeError("arg must be list")
        encoded = _encode_items(self._typecode, list_val)
        self._buf.extend(encoded)

    def tounicode(self):
        if self._typecode != "u":
            raise ValueError("tounicode() may only be called on unicode type arrays")
        return _decode_unicode(self._buf)

    def fromunicode(self, ustr, /):
        if self._typecode != "u":
            raise ValueError("fromunicode() may only be called on unicode type arrays")
        if not isinstance(ustr, str):
            raise TypeError(
                "fromunicode() argument must be str, not " + type(ustr).__name__
            )
        self._buf.extend(_encode_unicode(ustr))

    def tofile(self, f, /):
        f.write(self.tobytes())

    def fromfile(self, f, n, /):
        count = _as_index(n)
        if count < 0:
            raise ValueError("negative count")
        if count == 0:
            return
        nbytes = count * self._itemsize
        data = f.read(nbytes)
        if not isinstance(data, (bytes, bytearray)):
            raise TypeError("read() didn't return bytes")
        self.frombytes(data)
        if len(data) < nbytes:
            raise EOFError("read() didn't return enough bytes")

    def buffer_info(self):
        return (id(self._buf), len(self))

    def __iter__(self):
        idx = 0
        while idx < len(self):
            yield _decode_item(self._typecode, self._buf, idx)
            idx += 1

    def __reversed__(self):
        idx = len(self) - 1
        while idx >= 0:
            yield _decode_item(self._typecode, self._buf, idx)
            idx -= 1

    def __contains__(self, v):
        for idx in range(len(self)):
            if _decode_item(self._typecode, self._buf, idx) == v:
                return True
        return False

    def __getitem__(self, key):
        n = len(self)
        if isinstance(key, slice):
            start, stop, step = key.indices(n)
            sz = self._itemsize
            res = array(self._typecode)
            if step == 1:
                if start < stop:
                    res._buf.extend(self._buf[start * sz : stop * sz])
            else:
                for idx in range(start, stop, step):
                    pos = idx * sz
                    res._buf.extend(self._buf[pos : pos + sz])
            return res
        idx = _as_index(key)
        if idx < 0:
            idx += n
        if idx < 0 or idx >= n:
            raise IndexError("array index out of range")
        return _decode_item(self._typecode, self._buf, idx)

    def __setitem__(self, key, value):
        n = len(self)
        sz = self._itemsize
        if isinstance(key, slice):
            if not isinstance(value, array):
                raise TypeError(
                    'can only assign array (not "'
                    + type(value).__name__
                    + '") to array slice'
                )
            if value._typecode != self._typecode:
                raise TypeError("bad argument type for built-in operation")
            src = bytes(value._buf) if value is self else value._buf
            start, stop, step = key.indices(n)
            if step == 1:
                self._buf[start * sz : stop * sz] = src
            else:
                indices = list(range(start, stop, step))
                src_len = len(src) // sz
                if src_len != len(indices):
                    raise ValueError(
                        "attempt to assign array of size "
                        + str(src_len)
                        + " to extended slice of size "
                        + str(len(indices))
                    )
                for j, idx in enumerate(indices):
                    pos = idx * sz
                    spos = j * sz
                    self._buf[pos : pos + sz] = src[spos : spos + sz]
            return
        idx = _as_index(key)
        if idx < 0:
            idx += n
        if idx < 0 or idx >= n:
            raise IndexError("array assignment index out of range")
        encoded = _encode_item(self._typecode, value)
        pos = idx * sz
        self._buf[pos : pos + sz] = encoded

    def __delitem__(self, key):
        n = len(self)
        sz = self._itemsize
        if isinstance(key, slice):
            start, stop, step = key.indices(n)
            if step == 1:
                if start < stop:
                    del self._buf[start * sz : stop * sz]
            else:
                indices = list(range(start, stop, step))
                if step > 0:
                    indices.reverse()
                for idx in indices:
                    pos = idx * sz
                    del self._buf[pos : pos + sz]
            return
        idx = _as_index(key)
        if idx < 0:
            idx += n
        if idx < 0 or idx >= n:
            raise IndexError("array assignment index out of range")
        pos = idx * sz
        del self._buf[pos : pos + sz]

    def __add__(self, other):
        if not isinstance(other, array):
            raise TypeError(
                'can only append array (not "' + type(other).__name__ + '") to array'
            )
        if other._typecode != self._typecode:
            raise TypeError("bad argument type for built-in operation")
        res = array(self._typecode)
        res._buf.extend(self._buf)
        res._buf.extend(other._buf)
        return res

    def __iadd__(self, other):
        if not isinstance(other, array):
            raise TypeError(
                'can only extend array with array (not "'
                + type(other).__name__
                + '")'
            )
        self.extend(other)
        return self

    def __mul__(self, n):
        count = _as_index(n)
        res = array(self._typecode)
        if count > 0 and len(self._buf) > 0:
            res._buf.extend(bytes(self._buf) * count)
        return res

    def __rmul__(self, n):
        return self.__mul__(n)

    def __imul__(self, n):
        count = _as_index(n)
        if count <= 0:
            del self._buf[:]
        elif count > 1 and len(self._buf) > 0:
            chunk = bytes(self._buf)
            for _ in range(count - 1):
                self._buf.extend(chunk)
        return self

    def __eq__(self, other):
        if not isinstance(other, array):
            return NotImplemented
        n = len(self)
        if n != len(other):
            return False
        for i in range(n):
            if not (
                _decode_item(self._typecode, self._buf, i)
                == _decode_item(other._typecode, other._buf, i)
            ):
                return False
        return True

    def __ne__(self, other):
        eq = self.__eq__(other)
        if eq is NotImplemented:
            return NotImplemented
        return not eq

    def __lt__(self, other):
        if not isinstance(other, array):
            return NotImplemented
        na = len(self)
        nb = len(other)
        m = na if na < nb else nb
        for i in range(m):
            a = _decode_item(self._typecode, self._buf, i)
            b = _decode_item(other._typecode, other._buf, i)
            if a != b:
                return a < b
        return na < nb

    def __le__(self, other):
        if not isinstance(other, array):
            return NotImplemented
        na = len(self)
        nb = len(other)
        m = na if na < nb else nb
        for i in range(m):
            a = _decode_item(self._typecode, self._buf, i)
            b = _decode_item(other._typecode, other._buf, i)
            if a != b:
                return a <= b
        return na <= nb

    def __gt__(self, other):
        if not isinstance(other, array):
            return NotImplemented
        na = len(self)
        nb = len(other)
        m = na if na < nb else nb
        for i in range(m):
            a = _decode_item(self._typecode, self._buf, i)
            b = _decode_item(other._typecode, other._buf, i)
            if a != b:
                return a > b
        return na > nb

    def __ge__(self, other):
        if not isinstance(other, array):
            return NotImplemented
        na = len(self)
        nb = len(other)
        m = na if na < nb else nb
        for i in range(m):
            a = _decode_item(self._typecode, self._buf, i)
            b = _decode_item(other._typecode, other._buf, i)
            if a != b:
                return a >= b
        return na >= nb

    def __copy__(self):
        return array(self._typecode, bytes(self._buf))

    def __deepcopy__(self, memo):
        return array(self._typecode, bytes(self._buf))

    def __repr__(self):
        name = type(self).__name__
        if len(self._buf) == 0:
            return name + "(" + repr(self._typecode) + ")"
        if self._typecode == "u":
            return name + "('u', " + repr(self.tounicode()) + ")"
        return name + "(" + repr(self._typecode) + ", " + repr(self.tolist()) + ")"


ArrayType = array
