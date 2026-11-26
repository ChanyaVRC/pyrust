"""Basic message object for the email package object model (issue #3211)."""

import binascii
import random
import re
import sys
import time
from io import BytesIO, StringIO

from email import policy as _policy
from email import utils
from email.charset import Charset
from email.errors import HeaderParseError
from email.policy import compat32
from email.utils import (
    _decode_b_with_defects,
    _quopri_body_decode,
)

__all__ = ["Message", "EmailMessage"]

SEMISPACE = "; "
tspecials = re.compile(r'[ \(\)<>@,;:\\"/\[\]\?=]')
_FMT_BOUNDARY_WIDTH = len(repr(sys.maxsize - 1))
_FMT_BOUNDARY = "%%0%dd" % _FMT_BOUNDARY_WIDTH
_NLCRE = re.compile(r"\r\n|\r|\n")


def _splitparam(param):
    a, sep, b = str(param).partition(";")
    if not sep:
        return a.strip(), None
    return a.strip(), b.strip()


def _formatparam(param, value=None, quote=True):
    if value is not None and len(value) > 0:
        if isinstance(value, tuple):
            param += "*"
            value = utils.encode_rfc2231(value[2], value[0], value[1])
            return "%s=%s" % (param, value)
        try:
            value.encode("ascii")
        except UnicodeEncodeError:
            param += "*"
            value = utils.encode_rfc2231(value, "utf-8", "")
            return "%s=%s" % (param, value)
        if quote or tspecials.search(value):
            return '%s="%s"' % (param, utils.quote(value))
        return "%s=%s" % (param, value)
    return param


def _parseparam(s):
    s = ";" + str(s)
    plist = []
    while s[:1] == ";":
        s = s[1:]
        end = s.find(";")
        while end > 0 and (s.count('"', 0, end) - s.count('\\"', 0, end)) % 2:
            end = s.find(";", end + 1)
        if end < 0:
            end = len(s)
        f = s[:end]
        if "=" in f:
            i = f.index("=")
            f = f[:i].strip().lower() + "=" + f[i + 1 :].strip()
        plist.append(f.strip())
        s = s[end:]
    return plist


def _unquotevalue(value):
    if isinstance(value, tuple):
        return value[0], value[1], utils.unquote(value[2])
    return utils.unquote(value)


def _decode_uu(encoded):
    """Decode uuencoded data."""
    decoded_lines = []
    encoded_lines_iter = iter(encoded.splitlines())
    for line in encoded_lines_iter:
        if line.startswith(b"begin "):
            mode, _, _path = line.removeprefix(b"begin ").partition(b" ")
            try:
                int(mode, base=8)
            except ValueError:
                continue
            else:
                break
    else:
        raise ValueError("`begin` line not found")
    for line in encoded_lines_iter:
        if not line:
            raise ValueError("Truncated input")
        if line.strip(b" \t\r\n\f") == b"end":
            break
        try:
            decoded_line = binascii.a2b_uu(line)
        except binascii.Error:
            nbytes = (((line[0] - 32) & 63) * 4 + 5) // 3
            decoded_line = binascii.a2b_uu(line[:nbytes])
        decoded_lines.append(decoded_line)
    return b"".join(decoded_lines)


def _make_boundary(text=None):
    """Generate a unique MIME multipart boundary."""
    token = random.randrange(sys.maxsize)
    boundary = ("=" * 15) + (_FMT_BOUNDARY % token) + "=="
    if text is None:
        return boundary
    b = boundary
    counter = 0
    while True:
        cre = re.compile("^--" + re.escape(b) + "(--)?$", re.MULTILINE)
        if not cre.search(text):
            break
        b = boundary + "." + str(counter)
        counter += 1
    return b


def _flatten_to_str(msg, unixfrom=False, maxheaderlen=None, policy=None):
    """Serialize a Message tree into a string."""
    if policy is None:
        policy = msg.policy
    if maxheaderlen is not None:
        policy = policy.clone(max_line_length=maxheaderlen)
    linesep = policy.linesep
    out = StringIO()

    if unixfrom:
        ufrom = msg.get_unixfrom()
        if not ufrom:
            ufrom = "From nobody " + time.ctime(time.time())
        out.write(ufrom + linesep)

    if msg.is_multipart():
        # Serialize subparts first so we can ensure a collision-free boundary
        subparts = msg.get_payload() or []
        if msg.get_content_maintype() == "multipart":
            rendered_subparts = [
                _flatten_to_str(part, unixfrom=False, policy=policy)
                for part in subparts
            ]
            boundary = msg.get_boundary()
            if boundary is None:
                all_text = linesep.join(rendered_subparts)
                boundary = _make_boundary(all_text)
                if "content-type" not in msg:
                    msg["Content-Type"] = 'multipart/mixed; boundary="%s"' % boundary
                else:
                    msg.set_boundary(boundary)
            for h, v in msg.raw_items():
                out.write(policy.fold(h, v))
            out.write(linesep)
            if msg.preamble is not None:
                out.write(msg.preamble + linesep)
            for idx, body_text in enumerate(rendered_subparts):
                if idx == 0:
                    out.write("--" + boundary + linesep)
                else:
                    out.write(linesep + "--" + boundary + linesep)
                out.write(body_text)
            if rendered_subparts:
                out.write(linesep + "--" + boundary + "--")
            else:
                out.write("--" + boundary + "--")
            if msg.epilogue is not None:
                out.write(linesep + msg.epilogue)
            return out.getvalue()
        # Non-multipart container (e.g. message/rfc822)
        for h, v in msg.raw_items():
            out.write(policy.fold(h, v))
        out.write(linesep)
        for part in subparts:
            out.write(_flatten_to_str(part, unixfrom=False, policy=policy))
        return out.getvalue()

    for h, v in msg.raw_items():
        out.write(policy.fold(h, v))
    out.write(linesep)

    payload = msg.get_payload()
    if payload is None:
        return out.getvalue()
    if not isinstance(payload, str):
        raise TypeError(
            "string payload expected: %s" % type(payload)
        )
    if utils._has_surrogates(payload):
        payload = utils._sanitize(payload)
    if linesep != "\n":
        payload = _NLCRE.sub(linesep, payload)
    out.write(payload)
    return out.getvalue()


def _flatten_to_bytes(msg, unixfrom=False, policy=None):
    """Serialize a Message tree into bytes."""
    if policy is None:
        policy = msg.policy
    linesep_str = policy.linesep
    linesep = linesep_str.encode("ascii")
    out = BytesIO()

    if unixfrom:
        ufrom = msg.get_unixfrom()
        if not ufrom:
            ufrom = "From nobody " + time.ctime(time.time())
        out.write(ufrom.encode("ascii", "surrogateescape") + linesep)

    if msg.is_multipart():
        subparts = msg.get_payload() or []
        if msg.get_content_maintype() == "multipart":
            rendered_subparts = [
                _flatten_to_bytes(part, unixfrom=False, policy=policy)
                for part in subparts
            ]
            boundary = msg.get_boundary()
            if boundary is None:
                all_text = "\n".join(
                    p.decode("ascii", "surrogateescape") for p in rendered_subparts
                )
                boundary = _make_boundary(all_text)
                if "content-type" not in msg:
                    msg["Content-Type"] = 'multipart/mixed; boundary="%s"' % boundary
                else:
                    msg.set_boundary(boundary)
            for h, v in msg.raw_items():
                out.write(policy.fold_binary(h, v))
            out.write(linesep)
            b_boundary = boundary.encode("ascii")
            if msg.preamble is not None:
                out.write(
                    msg.preamble.encode("ascii", "surrogateescape") + linesep
                )
            for idx, body_bytes in enumerate(rendered_subparts):
                if idx == 0:
                    out.write(b"--" + b_boundary + linesep)
                else:
                    out.write(linesep + b"--" + b_boundary + linesep)
                out.write(body_bytes)
            if rendered_subparts:
                out.write(linesep + b"--" + b_boundary + b"--")
            else:
                out.write(b"--" + b_boundary + b"--")
            if msg.epilogue is not None:
                out.write(
                    linesep + msg.epilogue.encode("ascii", "surrogateescape")
                )
            return out.getvalue()
        for h, v in msg.raw_items():
            out.write(policy.fold_binary(h, v))
        out.write(linesep)
        for part in subparts:
            out.write(_flatten_to_bytes(part, unixfrom=False, policy=policy))
        return out.getvalue()

    for h, v in msg.raw_items():
        out.write(policy.fold_binary(h, v))
    out.write(linesep)

    payload = msg._payload
    if payload is None:
        return out.getvalue()
    if isinstance(payload, bytes):
        bpayload = payload
    elif isinstance(payload, str):
        if linesep_str != "\n":
            payload = _NLCRE.sub(linesep_str, payload)
        try:
            bpayload = payload.encode("ascii", "surrogateescape")
        except UnicodeEncodeError:
            bpayload = payload.encode("utf-8", "surrogateescape")
    else:
        raise TypeError("string payload expected: %s" % type(payload))
    out.write(bpayload)
    return out.getvalue()


class Message:
    """Basic RFC 2822 / MIME message object."""

    def __init__(self, policy=compat32):
        self.policy = policy
        self._headers = []
        self._unixfrom = None
        self._payload = None
        self._charset = None
        self.preamble = self.epilogue = None
        self.defects = []
        self._default_type = "text/plain"

    def __str__(self):
        return self.as_string()

    def as_string(self, unixfrom=False, maxheaderlen=0, policy=None):
        policy = self.policy if policy is None else policy
        return _flatten_to_str(
            self, unixfrom=unixfrom, maxheaderlen=maxheaderlen, policy=policy
        )

    def __bytes__(self):
        return self.as_bytes()

    def as_bytes(self, unixfrom=False, policy=None):
        policy = self.policy if policy is None else policy
        return _flatten_to_bytes(self, unixfrom=unixfrom, policy=policy)

    def is_multipart(self):
        return isinstance(self._payload, list)

    def set_unixfrom(self, unixfrom):
        self._unixfrom = unixfrom

    def get_unixfrom(self):
        return self._unixfrom

    def attach(self, payload):
        if self._payload is None:
            self._payload = [payload]
        else:
            try:
                self._payload.append(payload)
            except AttributeError:
                raise TypeError(
                    "Attach is not valid on a message with a non-multipart payload"
                )

    def get_payload(self, i=None, decode=False):
        if self.is_multipart():
            if decode:
                return None
            if i is None:
                return self._payload
            return self._payload[i]
        if i is not None and not isinstance(self._payload, list):
            raise TypeError("Expected list, got %s" % type(self._payload))
        payload = self._payload
        cte = str(self.get("content-transfer-encoding", "")).lower()
        if not decode:
            if isinstance(payload, str) and utils._has_surrogates(payload):
                try:
                    bpayload = payload.encode("ascii", "surrogateescape")
                    try:
                        payload = bpayload.decode(
                            self.get_param("charset", "ascii"), "replace"
                        )
                    except LookupError:
                        payload = bpayload.decode("ascii", "replace")
                except UnicodeEncodeError:
                    pass
            return payload
        if isinstance(payload, str):
            try:
                bpayload = payload.encode("ascii", "surrogateescape")
            except UnicodeEncodeError:
                bpayload = payload.encode("raw-unicode-escape")
        elif isinstance(payload, bytes):
            bpayload = payload
        else:
            return payload
        if cte == "quoted-printable":
            return _quopri_body_decode(bpayload)
        if cte == "base64":
            value, defects = _decode_b_with_defects(
                b"".join(bpayload.splitlines())
            )
            for defect in defects:
                self.policy.handle_defect(self, defect)
            return value
        if cte in ("x-uuencode", "uuencode", "uue", "x-uue"):
            try:
                return _decode_uu(bpayload)
            except ValueError:
                return bpayload
        if isinstance(payload, str):
            return bpayload
        return payload

    def set_payload(self, payload, charset=None):
        if hasattr(payload, "encode"):
            if charset is None:
                self._payload = payload
                return
            if not isinstance(charset, Charset):
                charset = Charset(charset)
            out_cs = charset.output_charset or "ascii"
            if out_cs.lower().replace("_", "-") == "us-ascii":
                out_cs = "ascii"
            payload = payload.encode(out_cs, "surrogateescape")
        if hasattr(payload, "decode"):
            self._payload = payload.decode("ascii", "surrogateescape")
        else:
            self._payload = payload
        if charset is not None:
            self.set_charset(charset)

    def set_charset(self, charset):
        if charset is None:
            self.del_param("charset")
            self._charset = None
            return
        if not isinstance(charset, Charset):
            charset = Charset(charset)
        self._charset = charset
        if "MIME-Version" not in self:
            self.add_header("MIME-Version", "1.0")
        if "Content-Type" not in self:
            self.add_header(
                "Content-Type",
                "text/plain",
                charset=charset.get_output_charset(),
            )
        else:
            self.set_param("charset", charset.get_output_charset())
        if charset != charset.get_output_charset():
            self._payload = charset.body_encode(self._payload)
        if "Content-Transfer-Encoding" not in self:
            cte = charset.get_body_encoding()
            if callable(cte):
                cte(self)
            elif cte == "7or8bit":
                payload = self._payload
                if payload is None:
                    self["Content-Transfer-Encoding"] = "7bit"
                else:
                    try:
                        if isinstance(payload, str):
                            payload.encode("ascii")
                        else:
                            payload.decode("ascii")
                        self["Content-Transfer-Encoding"] = "7bit"
                    except UnicodeError:
                        self["Content-Transfer-Encoding"] = "8bit"
            else:
                payload = self._payload
                if payload:
                    try:
                        payload = payload.encode("ascii", "surrogateescape")
                    except UnicodeError:
                        out_cs = charset.output_charset or "ascii"
                        if out_cs.lower().replace("_", "-") == "us-ascii":
                            out_cs = "ascii"
                        payload = payload.encode(out_cs)
                self._payload = charset.body_encode(payload)
                self.add_header("Content-Transfer-Encoding", cte)

    def get_charset(self):
        return self._charset

    # ── Mapping interface (case-insensitive header names) ────────────────────

    def __len__(self):
        return len(self._headers)

    def __getitem__(self, name):
        return self.get(name)

    def __setitem__(self, name, val):
        max_count = self.policy.header_max_count(name)
        if max_count:
            lname = name.lower()
            found = 0
            for k, _v in self._headers:
                if k.lower() == lname:
                    found += 1
                    if found >= max_count:
                        raise ValueError(
                            "There may be at most {} {} headers in a message".format(
                                max_count, name
                            )
                        )
        self._headers.append(self.policy.header_store_parse(name, val))

    def __delitem__(self, name):
        name = name.lower()
        self._headers = [
            (k, v) for k, v in self._headers if k.lower() != name
        ]

    def __contains__(self, name):
        name_lower = name.lower()
        for k, _v in self._headers:
            if name_lower == k.lower():
                return True
        return False

    def __iter__(self):
        for field, _value in self._headers:
            yield field

    def keys(self):
        return [k for k, _v in self._headers]

    def values(self):
        return [
            self.policy.header_fetch_parse(k, v) for k, v in self._headers
        ]

    def items(self):
        return [
            (k, self.policy.header_fetch_parse(k, v))
            for k, v in self._headers
        ]

    def get(self, name, failobj=None):
        name = name.lower()
        for k, v in self._headers:
            if k.lower() == name:
                return self.policy.header_fetch_parse(k, v)
        return failobj

    def set_raw(self, name, value):
        self._headers.append((name, value))

    def raw_items(self):
        return iter(self._headers.copy())

    def get_all(self, name, failobj=None):
        values = []
        name = name.lower()
        for k, v in self._headers:
            if k.lower() == name:
                values.append(self.policy.header_fetch_parse(k, v))
        if not values:
            return failobj
        return values

    def add_header(self, _name, _value, **_params):
        parts = []
        for k, v in _params.items():
            if v is None:
                parts.append(k.replace("_", "-"))
            else:
                parts.append(_formatparam(k.replace("_", "-"), v))
        if _value is not None:
            parts.insert(0, _value)
        self[_name] = SEMISPACE.join(parts)

    def replace_header(self, _name, _value):
        _name = _name.lower()
        for i, (k, _v) in enumerate(self._headers):
            if k.lower() == _name:
                self._headers[i] = self.policy.header_store_parse(k, _value)
                break
        else:
            raise KeyError(_name)

    # ── Content-Type & MIME parameter helpers ────────────────────────────────

    def get_content_type(self):
        missing = object()
        value = self.get("content-type", missing)
        if value is missing:
            return self.get_default_type()
        ctype = _splitparam(value)[0].lower()
        if ctype.count("/") != 1:
            return "text/plain"
        return ctype

    def get_content_maintype(self):
        return self.get_content_type().split("/")[0]

    def get_content_subtype(self):
        return self.get_content_type().split("/")[1]

    def get_default_type(self):
        return self._default_type

    def set_default_type(self, ctype):
        self._default_type = ctype

    def _get_params_preserve(self, failobj, header):
        missing = object()
        value = self.get(header, missing)
        if value is missing:
            return failobj
        params = []
        for p in _parseparam(value):
            try:
                name, val = p.split("=", 1)
                name = name.strip()
                val = val.strip()
            except ValueError:
                name = p.strip()
                val = ""
            params.append((name, val))
        return utils.decode_params(params)

    def get_params(self, failobj=None, header="content-type", unquote=True):
        missing = object()
        params = self._get_params_preserve(missing, header)
        if params is missing:
            return failobj
        if unquote:
            return [(k, _unquotevalue(v)) for k, v in params]
        return params

    def get_param(
        self, param, failobj=None, header="content-type", unquote=True
    ):
        if header not in self:
            return failobj
        for k, v in self._get_params_preserve(failobj, header):
            if k.lower() == param.lower():
                if unquote:
                    return _unquotevalue(v)
                return v
        return failobj

    def set_param(
        self,
        param,
        value,
        header="Content-Type",
        requote=True,
        charset=None,
        language="",
        replace=False,
    ):
        if not isinstance(value, tuple) and charset:
            value = (charset, language, value)

        if header not in self and header.lower() == "content-type":
            ctype = "text/plain"
        else:
            ctype = self.get(header)
        if not self.get_param(param, header=header):
            if not ctype:
                ctype = _formatparam(param, value, requote)
            else:
                ctype = SEMISPACE.join(
                    [ctype, _formatparam(param, value, requote)]
                )
        else:
            ctype = ""
            for old_param, old_value in self.get_params(
                header=header, unquote=requote
            ):
                if old_param.lower() == param.lower():
                    append_param = _formatparam(param, value, requote)
                else:
                    append_param = _formatparam(old_param, old_value, requote)
                if not ctype:
                    ctype = append_param
                else:
                    ctype = SEMISPACE.join([ctype, append_param])
        if ctype != self.get(header):
            if replace:
                self.replace_header(header, ctype)
            else:
                del self[header]
                self[header] = ctype

    def del_param(self, param, header="content-type", requote=True):
        if header not in self:
            return
        new_ctype = ""
        for p, v in self.get_params(header=header, unquote=requote):
            if p.lower() != param.lower():
                if not new_ctype:
                    new_ctype = _formatparam(p, v, requote)
                else:
                    new_ctype = SEMISPACE.join(
                        [new_ctype, _formatparam(p, v, requote)]
                    )
        if new_ctype != self.get(header):
            del self[header]
            self[header] = new_ctype

    def set_type(self, type, header="Content-Type", requote=True):
        if not type.count("/") == 1:
            raise ValueError
        if header.lower() == "content-type":
            del self["mime-version"]
            self["MIME-Version"] = "1.0"
        if header not in self:
            self[header] = type
            return
        params = self.get_params(header=header, unquote=requote)
        del self[header]
        self[header] = type
        for p, v in params[1:]:
            self.set_param(p, v, header, requote)

    def get_filename(self, failobj=None):
        missing = object()
        filename = self.get_param("filename", missing, "content-disposition")
        if filename is missing:
            filename = self.get_param("name", missing, "content-type")
        if filename is missing:
            return failobj
        return utils.collapse_rfc2231_value(filename).strip()

    def get_boundary(self, failobj=None):
        missing = object()
        boundary = self.get_param("boundary", missing)
        if boundary is missing:
            return failobj
        return utils.collapse_rfc2231_value(boundary).rstrip()

    def set_boundary(self, boundary):
        missing = object()
        params = self._get_params_preserve(missing, "content-type")
        if params is missing:
            raise HeaderParseError("No Content-Type header found")
        newparams = []
        foundp = False
        for pk, pv in params:
            if pk.lower() == "boundary":
                newparams.append(("boundary", '"%s"' % boundary))
                foundp = True
            else:
                newparams.append((pk, pv))
        if not foundp:
            newparams.append(("boundary", '"%s"' % boundary))
        newheaders = []
        for h, v in self._headers:
            if h.lower() == "content-type":
                parts = []
                for k, val in newparams:
                    if val == "":
                        parts.append(k)
                    else:
                        parts.append("%s=%s" % (k, val))
                val_str = SEMISPACE.join(parts)
                newheaders.append(self.policy.header_store_parse(h, val_str))
            else:
                newheaders.append((h, v))
        self._headers = newheaders

    def get_content_charset(self, failobj=None):
        missing = object()
        charset = self.get_param("charset", missing)
        if charset is missing:
            return failobj
        if isinstance(charset, tuple):
            pcharset = charset[0] or "ascii"
            if pcharset.lower().replace("_", "-") == "us-ascii":
                pcharset = "ascii"
            try:
                as_bytes = charset[2].encode("raw-unicode-escape")
                charset = str(as_bytes, pcharset)
            except (LookupError, UnicodeError):
                charset = charset[2]
        try:
            charset.encode("ascii")
        except UnicodeError:
            return failobj
        return charset.lower()

    def get_charsets(self, failobj=None):
        return [part.get_content_charset(failobj) for part in self.walk()]

    def get_content_disposition(self):
        value = self.get("content-disposition")
        if value is None:
            return None
        return _splitparam(value)[0].lower()

    def walk(self):
        """Walk over the message tree, yielding each subpart depth-first."""
        yield self
        if self.is_multipart():
            for subpart in self.get_payload():
                yield from subpart.walk()


class MIMEPart(Message):
    """Modern MIME part class with high-level content/multipart helpers."""

    def __init__(self, policy=None):
        if policy is None:
            policy = _policy.default
        super().__init__(policy)

    def as_string(self, unixfrom=False, maxheaderlen=None, policy=None):
        policy = self.policy if policy is None else policy
        if maxheaderlen is None:
            maxheaderlen = policy.max_line_length
        return super().as_string(unixfrom, maxheaderlen, policy)

    def __str__(self):
        return self.as_string(policy=self.policy.clone(utf8=True))

    def is_attachment(self):
        c_d = self.get_content_disposition()
        return c_d == "attachment"

    def _find_body(self, part, preferencelist):
        if part.is_attachment():
            return
        maintype, subtype = part.get_content_type().split("/")
        if maintype == "text":
            if subtype in preferencelist:
                yield (preferencelist.index(subtype), part)
            return
        if maintype != "multipart" or not self.is_multipart():
            return
        if subtype != "related":
            for subpart in part.iter_parts():
                yield from self._find_body(subpart, preferencelist)
            return
        if "related" in preferencelist:
            yield (preferencelist.index("related"), part)
        candidate = None
        start = part.get_param("start")
        if start:
            for subpart in part.iter_parts():
                if subpart["content-id"] == start:
                    candidate = subpart
                    break
        if candidate is None:
            subparts = part.get_payload()
            candidate = subparts[0] if subparts else None
        if candidate is not None:
            yield from self._find_body(candidate, preferencelist)

    def get_body(self, preferencelist=("related", "html", "plain")):
        best_prio = len(preferencelist)
        body = None
        for prio, part in self._find_body(self, preferencelist):
            if prio < best_prio:
                best_prio = prio
                body = part
                if prio == 0:
                    break
        return body

    _body_types = {
        ("text", "plain"),
        ("text", "html"),
        ("multipart", "related"),
        ("multipart", "alternative"),
    }

    def iter_attachments(self):
        maintype, subtype = self.get_content_type().split("/")
        if maintype != "multipart" or subtype == "alternative":
            return
        payload = self.get_payload()
        try:
            parts = payload.copy()
        except AttributeError:
            return

        if maintype == "multipart" and subtype == "related":
            start = self.get_param("start")
            if start:
                found = False
                attachments = []
                for part in parts:
                    if part.get("content-id") == start:
                        found = True
                    else:
                        attachments.append(part)
                if found:
                    yield from attachments
                    return
            if parts:
                parts.pop(0)
            yield from parts
            return
        seen = []
        for part in parts:
            mt, st = part.get_content_type().split("/")
            if (
                (mt, st) in self._body_types
                and not part.is_attachment()
                and st not in seen
            ):
                seen.append(st)
                continue
            yield part

    def iter_parts(self):
        if self.is_multipart():
            yield from self.get_payload()

    def get_content(self, *args, content_manager=None, **kw):
        if content_manager is None:
            content_manager = self.policy.content_manager
        return content_manager.get_content(self, *args, **kw)

    def set_content(self, *args, content_manager=None, **kw):
        if content_manager is None:
            content_manager = self.policy.content_manager
        content_manager.set_content(self, *args, **kw)

    def _make_multipart(self, subtype, disallowed_subtypes, boundary):
        if self.get_content_maintype() == "multipart":
            existing_subtype = self.get_content_subtype()
            disallowed_subtypes = disallowed_subtypes + (subtype,)
            if existing_subtype in disallowed_subtypes:
                raise ValueError(
                    "Cannot convert {} to {}".format(existing_subtype, subtype)
                )
        keep_headers = []
        part_headers = []
        for name, value in self._headers:
            if name.lower().startswith("content-"):
                part_headers.append((name, value))
            else:
                keep_headers.append((name, value))
        if part_headers:
            part = type(self)(policy=self.policy)
            part._headers = part_headers
            part._payload = self._payload
            self._payload = [part]
        else:
            self._payload = []
        self._headers = keep_headers
        self["Content-Type"] = "multipart/" + subtype
        if boundary is not None:
            self.set_param("boundary", boundary)

    def make_related(self, boundary=None):
        self._make_multipart("related", ("alternative", "mixed"), boundary)

    def make_alternative(self, boundary=None):
        self._make_multipart("alternative", ("mixed",), boundary)

    def make_mixed(self, boundary=None):
        self._make_multipart("mixed", (), boundary)

    def _add_multipart(self, _subtype, *args, _disp=None, **kw):
        if (
            self.get_content_maintype() != "multipart"
            or self.get_content_subtype() != _subtype
        ):
            getattr(self, "make_" + _subtype)()
        part = type(self)(policy=self.policy)
        part.set_content(*args, **kw)
        if _disp and "content-disposition" not in part:
            part["Content-Disposition"] = _disp
        self.attach(part)

    def add_related(self, *args, **kw):
        self._add_multipart("related", *args, _disp="inline", **kw)

    def add_alternative(self, *args, **kw):
        self._add_multipart("alternative", *args, **kw)

    def add_attachment(self, *args, **kw):
        self._add_multipart("mixed", *args, _disp="attachment", **kw)

    def clear(self):
        self._headers = []
        self._payload = None

    def clear_content(self):
        self._headers = [
            (n, v)
            for n, v in self._headers
            if not n.lower().startswith("content-")
        ]
        self._payload = None


class EmailMessage(MIMEPart):
    """High-level email message using email.policy.default."""

    def set_content(self, *args, **kw):
        super().set_content(*args, **kw)
        if "MIME-Version" not in self:
            self["MIME-Version"] = "1.0"
