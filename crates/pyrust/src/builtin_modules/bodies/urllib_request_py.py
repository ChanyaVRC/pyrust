"""An extensible library for opening URLs using a variety of protocols."""

import base64
import bisect
import contextlib
import hashlib
import io
import os
import re
import socket
import string
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.response
import warnings

try:
    import ssl
except ImportError:
    _have_ssl = False
else:
    _have_ssl = True

__all__ = [
    # Classes
    'Request', 'OpenerDirector', 'BaseHandler', 'HTTPDefaultErrorHandler',
    'HTTPRedirectHandler', 'ProxyHandler',
    'HTTPPasswordMgr', 'HTTPPasswordMgrWithDefaultRealm',
    'HTTPPasswordMgrWithPriorAuth', 'AbstractBasicAuthHandler',
    'HTTPBasicAuthHandler', 'ProxyBasicAuthHandler', 'AbstractDigestAuthHandler',
    'HTTPDigestAuthHandler', 'ProxyDigestAuthHandler', 'HTTPHandler',
    'FileHandler', 'DataHandler',
    'UnknownHandler', 'HTTPErrorProcessor',
    # Functions
    'urlopen', 'install_opener', 'build_opener',
    'pathname2url', 'url2pathname', 'getproxies',
    # Legacy interface
    'urlretrieve', 'urlcleanup',
]

# used in User-Agent header sent
__version__ = '%d.%d' % sys.version_info[:2]


# ── Non-deprecated URL splitting helpers (mirrors urllib.parse._split*) ──────

_typeprog = re.compile('([^/:]+):(.*)', re.DOTALL)
_hostprog = re.compile('//([^/#?]*)(.*)', re.DOTALL)
_portprog = re.compile('(.*):([0-9]*)', re.DOTALL)


def _splittype(url):
    match = _typeprog.match(url)
    if match:
        scheme, data = match.groups()
        return scheme.lower(), data
    return None, url


def _splithost(url):
    match = _hostprog.match(url)
    if match:
        host_port, path = match.groups()
        if path and path[0] != '/':
            path = '/' + path
        return host_port, path
    return None, url


def _splituser(host):
    user, delim, host = host.rpartition('@')
    return (user if delim else None), host


def _splitpasswd(user):
    user, delim, passwd = user.partition(':')
    return user, (passwd if delim else None)


def _splitport(host):
    match = _portprog.fullmatch(host)
    if match:
        host, port = match.groups()
        if port:
            return host, port
    return host, None


def _splittag(url):
    path, delim, tag = url.rpartition('#')
    if delim:
        return path, tag
    return url, None


# ── Top-level opener / retrieval API ─────────────────────────────────────────

_opener = None


def urlopen(url, data=None, timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
            *, cafile=None, capath=None, cadefault=False, context=None):
    """Open the URL url, which can be either a string or a Request object."""
    global _opener
    if cafile or capath or cadefault:
        warnings.warn(
            "cafile, capath and cadefault are deprecated, use a "
            "custom context instead.",
            DeprecationWarning,
            2,
        )
        if context is not None:
            raise ValueError(
                "You can't pass both context and any of cafile, capath, and "
                "cadefault"
            )
        if not _have_ssl:
            raise ValueError('SSL support not available')
        context = ssl.create_default_context(
            ssl.Purpose.SERVER_AUTH,
            cafile=cafile,
            capath=capath,
        )
        context.set_alpn_protocols(['http/1.1'])
        https_handler = HTTPSHandler(context=context)
        opener = build_opener(https_handler)
    elif context:
        if not _have_ssl:
            raise ValueError('SSL support not available')
        https_handler = HTTPSHandler(context=context)
        opener = build_opener(https_handler)
    elif _opener is None:
        _opener = opener = build_opener()
    else:
        opener = _opener
    return opener.open(url, data, timeout)


def install_opener(opener):
    global _opener
    _opener = opener


_url_tempfiles = []


def urlretrieve(url, filename=None, reporthook=None, data=None):
    """Retrieve a URL into a temporary location on disk."""
    url_type, path = _splittype(url)

    with contextlib.closing(urlopen(url, data)) as fp:
        headers = fp.info()

        # Just return the local path and the "headers" for file://
        # URLs. No sense in performing a copy unless requested.
        if url_type == "file" and not filename:
            return os.path.normpath(path), headers

        # Handle temporary file setup.
        if filename:
            tfp = open(filename, 'wb')
        else:
            tfp = tempfile.NamedTemporaryFile(delete=False)
            filename = tfp.name
            _url_tempfiles.append(filename)

        with tfp:
            result = filename, headers
            bs = 1024 * 8
            size = -1
            read = 0
            blocknum = 0
            if "content-length" in headers:
                size = int(headers["Content-Length"])

            if reporthook:
                reporthook(blocknum, bs, size)

            while block := fp.read(bs):
                read += len(block)
                tfp.write(block)
                blocknum += 1
                if reporthook:
                    reporthook(blocknum, bs, size)

    if size >= 0 and read < size:
        raise urllib.error.ContentTooShortError(
            "retrieval incomplete: got only %i out of %i bytes"
            % (read, size),
            result,
        )

    return result


def urlcleanup():
    """Clean up temporary files from urlretrieve calls."""
    for temp_file in _url_tempfiles:
        try:
            os.unlink(temp_file)
        except OSError:
            pass

    del _url_tempfiles[:]
    global _opener
    if _opener:
        _opener = None


# ── Request ──────────────────────────────────────────────────────────────────

_cut_port_re = re.compile(r":\d+$", re.ASCII)


def request_host(request):
    """Return request-host, as defined by RFC 2965 (lowercased)."""
    url = request.full_url
    host = urllib.parse.urlparse(url)[1]
    if host == "":
        host = request.get_header("Host", "")

    # remove port, if present
    host = _cut_port_re.sub("", host, 1)
    return host.lower()


class Request:

    def __init__(self, url, data=None, headers={},
                 origin_req_host=None, unverifiable=False,
                 method=None):
        self.full_url = url
        self.headers = {}
        self.unredirected_hdrs = {}
        self._data = None
        self.data = data
        self._tunnel_host = None
        for key, value in headers.items():
            self.add_header(key, value)
        if origin_req_host is None:
            origin_req_host = request_host(self)
        self.origin_req_host = origin_req_host
        self.unverifiable = unverifiable
        if method:
            self.method = method

    @property
    def full_url(self):
        if self.fragment:
            return '{}#{}'.format(self._full_url, self.fragment)
        return self._full_url

    @full_url.setter
    def full_url(self, url):
        # unwrap('<URL:type://host/path>') --> 'type://host/path'
        self._full_url = urllib.parse.unwrap(url)
        self._full_url, self.fragment = _splittag(self._full_url)
        self._parse()

    @full_url.deleter
    def full_url(self):
        self._full_url = None
        self.fragment = None
        self.selector = ''

    @property
    def data(self):
        return self._data

    @data.setter
    def data(self, value):
        if value != self._data:
            self._data = value
            # if we change data we need to remove content-length header
            if self.has_header("Content-length"):
                self.remove_header("Content-length")

    @data.deleter
    def data(self):
        self.data = None

    def _parse(self):
        self.type, rest = _splittype(self._full_url)
        if self.type is None:
            raise ValueError("unknown url type: %r" % self.full_url)
        self.host, self.selector = _splithost(rest)
        if self.host:
            self.host = urllib.parse.unquote(self.host)

    def get_method(self):
        """Return a string indicating the HTTP request method."""
        default_method = "POST" if self.data is not None else "GET"
        return getattr(self, 'method', default_method)

    def get_full_url(self):
        return self.full_url

    def set_proxy(self, host, type):
        if self.type == 'https' and not self._tunnel_host:
            self._tunnel_host = self.host
        else:
            self.type = type
            self.selector = self.full_url
        self.host = host

    def has_proxy(self):
        return self.selector == self.full_url

    def add_header(self, key, val):
        self.headers[key.capitalize()] = val

    def add_unredirected_header(self, key, val):
        self.unredirected_hdrs[key.capitalize()] = val

    def has_header(self, header_name):
        return (
            header_name in self.headers
            or header_name in self.unredirected_hdrs
        )

    def get_header(self, header_name, default=None):
        return self.headers.get(
            header_name,
            self.unredirected_hdrs.get(header_name, default),
        )

    def remove_header(self, header_name):
        self.headers.pop(header_name, None)
        self.unredirected_hdrs.pop(header_name, None)

    def header_items(self):
        hdrs = {**self.unredirected_hdrs, **self.headers}
        return list(hdrs.items())


# ── OpenerDirector & build_opener ────────────────────────────────────────────

class OpenerDirector:
    def __init__(self):
        client_version = "Python-urllib/%s" % __version__
        self.addheaders = [('User-agent', client_version)]
        # self.handlers is retained only for backward compatibility
        self.handlers = []
        # manage the individual handlers
        self.handle_open = {}
        self.handle_error = {}
        self.process_response = {}
        self.process_request = {}

    def add_handler(self, handler):
        if not hasattr(handler, "add_parent"):
            raise TypeError(
                "expected BaseHandler instance, got %r" % type(handler)
            )

        added = False
        for meth in dir(handler):
            if meth in ["redirect_request", "do_open", "proxy_open"]:
                continue

            i = meth.find("_")
            if i < 0:
                continue
            protocol = meth[:i]
            condition = meth[i + 1:]

            if condition.startswith("error"):
                j = condition.find("_") + i + 1
                kind = meth[j + 1:]
                try:
                    kind = int(kind)
                except ValueError:
                    pass
                lookup = self.handle_error.get(protocol, {})
                self.handle_error[protocol] = lookup
            elif condition == "open":
                kind = protocol
                lookup = self.handle_open
            elif condition == "response":
                kind = protocol
                lookup = self.process_response
            elif condition == "request":
                kind = protocol
                lookup = self.process_request
            else:
                continue

            handlers = lookup.setdefault(kind, [])
            if handlers:
                bisect.insort(handlers, handler)
            else:
                handlers.append(handler)
            added = True

        if added:
            bisect.insort(self.handlers, handler)
            handler.add_parent(self)

    def close(self):
        pass

    def _call_chain(self, chain, kind, meth_name, *args):
        handlers = chain.get(kind, ())
        for handler in handlers:
            func = getattr(handler, meth_name)
            result = func(*args)
            if result is not None:
                return result

    def open(self, fullurl, data=None, timeout=socket._GLOBAL_DEFAULT_TIMEOUT):
        if isinstance(fullurl, str):
            req = Request(fullurl, data)
        else:
            req = fullurl
            if data is not None:
                req.data = data

        req.timeout = timeout
        protocol = req.type

        # pre-process request
        meth_name = protocol + "_request"
        for processor in self.process_request.get(protocol, []):
            meth = getattr(processor, meth_name)
            req = meth(req)

        sys.audit(
            'urllib.Request',
            req.full_url,
            req.data,
            req.headers,
            req.get_method(),
        )
        response = self._open(req, data)

        # post-process response
        meth_name = protocol + "_response"
        for processor in self.process_response.get(protocol, []):
            meth = getattr(processor, meth_name)
            response = meth(req, response)

        return response

    def _open(self, req, data=None):
        result = self._call_chain(
            self.handle_open, 'default', 'default_open', req
        )
        if result:
            return result

        protocol = req.type
        result = self._call_chain(
            self.handle_open, protocol, protocol + '_open', req
        )
        if result:
            return result

        return self._call_chain(
            self.handle_open, 'unknown', 'unknown_open', req
        )

    def error(self, proto, *args):
        if proto in ('http', 'https'):
            err_dict = self.handle_error.get('http', {})
            proto = args[2]
            meth_name = 'http_error_%s' % proto
            http_err = 1
            orig_args = args
        else:
            err_dict = self.handle_error
            meth_name = proto + '_error'
            http_err = 0
        call_args = (err_dict, proto, meth_name) + args
        result = self._call_chain(*call_args)
        if result:
            return result

        if http_err:
            call_args = (err_dict, 'default', 'http_error_default') + orig_args
            return self._call_chain(*call_args)


def build_opener(*handlers):
    """Create an opener object from a list of handlers."""
    import http.client

    opener = OpenerDirector()
    default_classes = [
        ProxyHandler,
        UnknownHandler,
        HTTPHandler,
        HTTPDefaultErrorHandler,
        HTTPRedirectHandler,
        FileHandler,
        HTTPErrorProcessor,
        DataHandler,
    ]
    if hasattr(http.client, "HTTPSConnection"):
        default_classes.append(HTTPSHandler)
    skip = set()
    for klass in default_classes:
        for check in handlers:
            if isinstance(check, type):
                if issubclass(check, klass):
                    skip.add(klass)
            elif isinstance(check, klass):
                skip.add(klass)
    for klass in skip:
        default_classes.remove(klass)

    for klass in default_classes:
        opener.add_handler(klass())

    for h in handlers:
        if isinstance(h, type):
            h = h()
        opener.add_handler(h)
    return opener


# ── Handlers ─────────────────────────────────────────────────────────────────

class BaseHandler:
    handler_order = 500

    def add_parent(self, parent):
        self.parent = parent

    def close(self):
        pass

    def __lt__(self, other):
        if not hasattr(other, "handler_order"):
            return True
        return self.handler_order < other.handler_order


class HTTPErrorProcessor(BaseHandler):
    """Process HTTP error responses."""

    handler_order = 1000  # after all other processing

    def http_response(self, request, response):
        code, msg, hdrs = response.code, response.msg, response.info()

        # According to RFC 2616, "2xx" code indicates success.
        if not (200 <= code < 300):
            response = self.parent.error(
                'http', request, response, code, msg, hdrs
            )

        return response

    https_response = http_response


class HTTPDefaultErrorHandler(BaseHandler):
    def http_error_default(self, req, fp, code, msg, hdrs):
        raise urllib.error.HTTPError(req.full_url, code, msg, hdrs, fp)


class HTTPRedirectHandler(BaseHandler):
    max_repeats = 4
    max_redirections = 10

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        """Return a Request or None in response to a redirect."""
        m = req.get_method()
        if not (
            code in (301, 302, 303, 307, 308) and m in ("GET", "HEAD")
            or code in (301, 302, 303) and m == "POST"
        ):
            raise urllib.error.HTTPError(req.full_url, code, msg, headers, fp)

        newurl = newurl.replace(' ', '%20')

        CONTENT_HEADERS = ("content-length", "content-type")
        newheaders = {
            k: v
            for k, v in req.headers.items()
            if k.lower() not in CONTENT_HEADERS
        }
        return Request(
            newurl,
            headers=newheaders,
            origin_req_host=req.origin_req_host,
            unverifiable=True,
        )

    def http_error_302(self, req, fp, code, msg, headers):
        if "location" in headers:
            newurl = headers["location"]
        elif "uri" in headers:
            newurl = headers["uri"]
        else:
            return

        urlparts = urllib.parse.urlparse(newurl)

        if urlparts.scheme not in ('http', 'https', 'ftp', ''):
            raise urllib.error.HTTPError(
                newurl,
                code,
                "%s - Redirection to url '%s' is not allowed" % (msg, newurl),
                headers,
                fp,
            )

        if not urlparts.path and urlparts.netloc:
            urlparts = list(urlparts)
            urlparts[2] = "/"
        newurl = urllib.parse.urlunparse(urlparts)

        newurl = urllib.parse.quote(
            newurl, encoding="iso-8859-1", safe=string.punctuation
        )
        newurl = urllib.parse.urljoin(req.full_url, newurl)

        new = self.redirect_request(req, fp, code, msg, headers, newurl)
        if new is None:
            return

        # loop detection
        if hasattr(req, 'redirect_dict'):
            visited = new.redirect_dict = req.redirect_dict
            if (
                visited.get(newurl, 0) >= self.max_repeats
                or len(visited) >= self.max_redirections
            ):
                raise urllib.error.HTTPError(
                    req.full_url, code, self.inf_msg + msg, headers, fp
                )
        else:
            visited = new.redirect_dict = req.redirect_dict = {}
        visited[newurl] = visited.get(newurl, 0) + 1

        fp.read()
        fp.close()

        return self.parent.open(new, timeout=req.timeout)

    http_error_301 = http_error_303 = http_error_307 = http_error_308 = http_error_302

    inf_msg = (
        "The HTTP server returned a redirect error that would "
        "lead to an infinite loop.\n"
        "The last 30x error message was:\n"
    )


def _parse_proxy(proxy):
    """Return (scheme, user, password, host/port) given a URL or an authority."""
    scheme, r_scheme = _splittype(proxy)
    if not r_scheme.startswith("/"):
        scheme = None
        authority = proxy
    else:
        if not r_scheme.startswith("//"):
            raise ValueError("proxy URL with no authority: %r" % proxy)
        if '@' in r_scheme:
            host_separator = r_scheme.find('@')
            end = r_scheme.find("/", host_separator)
        else:
            end = r_scheme.find("/", 2)
        if end == -1:
            end = None
        authority = r_scheme[2:end]
    userinfo, hostport = _splituser(authority)
    if userinfo is not None:
        user, password = _splitpasswd(userinfo)
    else:
        user = password = None
    return scheme, user, password, hostport


class ProxyHandler(BaseHandler):
    handler_order = 100

    def __init__(self, proxies=None):
        if proxies is None:
            proxies = getproxies()
        assert hasattr(proxies, 'keys'), "proxies must be a mapping"
        self.proxies = proxies
        for type, url in proxies.items():
            type = type.lower()
            setattr(
                self,
                '%s_open' % type,
                lambda r, proxy=url, type=type, meth=self.proxy_open: meth(
                    r, proxy, type
                ),
            )

    def proxy_open(self, req, proxy, type):
        orig_type = req.type
        proxy_type, user, password, hostport = _parse_proxy(proxy)
        if proxy_type is None:
            proxy_type = orig_type

        if req.host and proxy_bypass(req.host):
            return None

        if user and password:
            user_pass = '%s:%s' % (
                urllib.parse.unquote(user),
                urllib.parse.unquote(password),
            )
            creds = base64.b64encode(user_pass.encode()).decode("ascii")
            req.add_header('Proxy-authorization', 'Basic ' + creds)
        hostport = urllib.parse.unquote(hostport)
        req.set_proxy(hostport, proxy_type)
        if orig_type == proxy_type or orig_type == 'https':
            return None
        else:
            return self.parent.open(req, timeout=req.timeout)


# ── Authentication Managers & Handlers ───────────────────────────────────────

class HTTPPasswordMgr:

    def __init__(self):
        self.passwd = {}

    def add_password(self, realm, uri, user, passwd):
        if isinstance(uri, str):
            uri = [uri]
        if realm not in self.passwd:
            self.passwd[realm] = {}
        for default_port in (True, False):
            reduced_uri = tuple(
                self.reduce_uri(u, default_port) for u in uri
            )
            self.passwd[realm][reduced_uri] = (user, passwd)

    def find_user_password(self, realm, authuri):
        domains = self.passwd.get(realm, {})
        for default_port in (True, False):
            reduced_authuri = self.reduce_uri(authuri, default_port)
            for uris, authinfo in domains.items():
                for uri in uris:
                    if self.is_suburi(uri, reduced_authuri):
                        return authinfo
        return None, None

    def reduce_uri(self, uri, default_port=True):
        """Accept authority or URI and extract only the authority and path."""
        parts = urllib.parse.urlsplit(uri)
        if parts[1]:
            scheme = parts[0]
            authority = parts[1]
            path = parts[2] or '/'
        else:
            scheme = None
            authority = uri
            path = '/'
        host, port = _splitport(authority)
        if default_port and port is None and scheme is not None:
            dport = {"http": 80, "https": 443}.get(scheme)
            if dport is not None:
                authority = "%s:%d" % (host, dport)
        return authority, path

    def is_suburi(self, base, test):
        """Check if test is below base in a URI tree."""
        if base == test:
            return True
        if base[0] != test[0]:
            return False
        prefix = base[1]
        if prefix[-1:] != '/':
            prefix += '/'
        return test[1].startswith(prefix)


class HTTPPasswordMgrWithDefaultRealm(HTTPPasswordMgr):

    def find_user_password(self, realm, authuri):
        user, password = HTTPPasswordMgr.find_user_password(
            self, realm, authuri
        )
        if user is not None:
            return user, password
        return HTTPPasswordMgr.find_user_password(self, None, authuri)


class HTTPPasswordMgrWithPriorAuth(HTTPPasswordMgrWithDefaultRealm):

    def __init__(self, *args, **kwargs):
        self.authenticated = {}
        super().__init__(*args, **kwargs)

    def add_password(self, realm, uri, user, passwd, is_authenticated=False):
        self.update_authenticated(uri, is_authenticated)
        if realm is not None:
            super().add_password(None, uri, user, passwd)
        super().add_password(realm, uri, user, passwd)

    def update_authenticated(self, uri, is_authenticated=False):
        if isinstance(uri, str):
            uri = [uri]
        for default_port in (True, False):
            for u in uri:
                reduced_uri = self.reduce_uri(u, default_port)
                self.authenticated[reduced_uri] = is_authenticated

    def is_authenticated(self, authuri):
        for default_port in (True, False):
            reduced_authuri = self.reduce_uri(authuri, default_port)
            for uri in self.authenticated:
                if self.is_suburi(uri, reduced_authuri):
                    return self.authenticated[uri]


class AbstractBasicAuthHandler:
    rx = re.compile(
        '(?:^|,)[ \t]*([^ \t,]+)[ \t]+realm=(["\']?)([^"\']*)\\2',
        re.I,
    )

    def __init__(self, password_mgr=None):
        if password_mgr is None:
            password_mgr = HTTPPasswordMgr()
        self.passwd = password_mgr
        self.add_password = self.passwd.add_password

    def _parse_realm(self, header):
        found_challenge = False
        for mo in AbstractBasicAuthHandler.rx.finditer(header):
            scheme, q, realm = mo.groups()
            if q not in ('"', "'"):
                warnings.warn("Basic Auth Realm was unquoted", UserWarning, 3)
            yield (scheme, realm)
            found_challenge = True

        if not found_challenge:
            scheme = header.split()[0] if header else ''
            yield (scheme, None)

    def http_error_auth_reqed(self, authreq, host, req, headers):
        headers = headers.get_all(authreq)
        if not headers:
            return

        unsupported = None
        for header in headers:
            for scheme, realm in self._parse_realm(header):
                if scheme.lower() != 'basic':
                    unsupported = scheme
                    continue
                if realm is not None:
                    return self.retry_http_basic_auth(host, req, realm)

        if unsupported is not None:
            raise ValueError(
                "AbstractBasicAuthHandler does not "
                "support the following scheme: %r" % (unsupported,)
            )

    def retry_http_basic_auth(self, host, req, realm):
        user, pw = self.passwd.find_user_password(realm, host)
        if pw is not None:
            raw = "%s:%s" % (user, pw)
            auth = "Basic " + base64.b64encode(raw.encode()).decode("ascii")
            if req.get_header(self.auth_header, None) == auth:
                return None
            req.add_unredirected_header(self.auth_header, auth)
            return self.parent.open(req, timeout=req.timeout)
        return None

    def http_request(self, req):
        if (
            not hasattr(self.passwd, 'is_authenticated')
            or not self.passwd.is_authenticated(req.full_url)
        ):
            return req

        if not req.has_header('Authorization'):
            user, passwd = self.passwd.find_user_password(None, req.full_url)
            credentials = '{0}:{1}'.format(user, passwd).encode()
            auth_str = base64.standard_b64encode(credentials).decode()
            req.add_unredirected_header(
                'Authorization', 'Basic {}'.format(auth_str.strip())
            )
        return req

    def http_response(self, req, response):
        if hasattr(self.passwd, 'is_authenticated'):
            if 200 <= response.code < 300:
                self.passwd.update_authenticated(req.full_url, True)
            else:
                self.passwd.update_authenticated(req.full_url, False)
        return response

    https_request = http_request
    https_response = http_response


class HTTPBasicAuthHandler(AbstractBasicAuthHandler, BaseHandler):
    auth_header = 'Authorization'

    def http_error_401(self, req, fp, code, msg, headers):
        url = req.full_url
        return self.http_error_auth_reqed('www-authenticate', url, req, headers)


class ProxyBasicAuthHandler(AbstractBasicAuthHandler, BaseHandler):
    auth_header = 'Proxy-authorization'

    def http_error_407(self, req, fp, code, msg, headers):
        authority = req.host
        return self.http_error_auth_reqed(
            'proxy-authenticate', authority, req, headers
        )


_randombytes = os.urandom


class AbstractDigestAuthHandler:

    def __init__(self, passwd=None):
        if passwd is None:
            passwd = HTTPPasswordMgr()
        self.passwd = passwd
        self.add_password = self.passwd.add_password
        self.retried = 0
        self.nonce_count = 0
        self.last_nonce = None

    def reset_retry_count(self):
        self.retried = 0

    def http_error_auth_reqed(self, auth_header, host, req, headers):
        authreq = headers.get(auth_header, None)
        if self.retried > 5:
            raise urllib.error.HTTPError(
                req.full_url, 401, "digest auth failed", headers, None
            )
        else:
            self.retried += 1
        if authreq:
            scheme = authreq.split()[0]
            if scheme.lower() == 'digest':
                return self.retry_http_digest_auth(req, authreq)
            elif scheme.lower() != 'basic':
                raise ValueError(
                    "AbstractDigestAuthHandler does not support"
                    " the following scheme: '%s'" % scheme
                )

    def retry_http_digest_auth(self, req, auth):
        token, challenge = auth.split(' ', 1)
        chal = parse_keqv_list(filter(None, parse_http_list(challenge)))
        auth = self.get_authorization(req, chal)
        if auth:
            auth_val = 'Digest %s' % auth
            if req.headers.get(self.auth_header, None) == auth_val:
                return None
            req.add_unredirected_header(self.auth_header, auth_val)
            return self.parent.open(req, timeout=req.timeout)

    def get_cnonce(self, nonce):
        s = "%s:%s:%s:" % (self.nonce_count, nonce, time.ctime())
        b = s.encode("ascii") + _randombytes(8)
        dig = hashlib.sha1(b).hexdigest()
        return dig[:16]

    def get_authorization(self, req, chal):
        try:
            realm = chal['realm']
            nonce = chal['nonce']
            qop = chal.get('qop')
            algorithm = chal.get('algorithm', 'MD5')
            opaque = chal.get('opaque', None)
        except KeyError:
            return None

        H, KD = self.get_algorithm_impls(algorithm)
        if H is None:
            return None

        user, pw = self.passwd.find_user_password(realm, req.full_url)
        if user is None:
            return None

        entdig = self.get_entity_digest(req.data, chal) if req.data is not None else None

        A1 = "%s:%s:%s" % (user, realm, pw)
        A2 = "%s:%s" % (req.get_method(), req.selector)
        if qop is None:
            respdig = KD(H(A1), "%s:%s" % (nonce, H(A2)))
        elif 'auth' in qop.split(','):
            if nonce == self.last_nonce:
                self.nonce_count += 1
            else:
                self.nonce_count = 1
                self.last_nonce = nonce
            ncvalue = '%08x' % self.nonce_count
            cnonce = self.get_cnonce(nonce)
            noncebit = "%s:%s:%s:%s:%s" % (nonce, ncvalue, cnonce, 'auth', H(A2))
            respdig = KD(H(A1), noncebit)
        else:
            raise urllib.error.URLError("qop '%s' is not supported." % qop)

        base = (
            'username="%s", realm="%s", nonce="%s", uri="%s", response="%s"'
            % (user, realm, nonce, req.selector, respdig)
        )
        if opaque:
            base += ', opaque="%s"' % opaque
        if entdig:
            base += ', digest="%s"' % entdig
        base += ', algorithm="%s"' % algorithm
        if qop:
            base += ', qop=auth, nc=%s, cnonce="%s"' % (ncvalue, cnonce)
        return base

    def get_algorithm_impls(self, algorithm):
        if algorithm == 'MD5':
            H = lambda x: hashlib.md5(x.encode("ascii")).hexdigest()
        elif algorithm == 'SHA':
            H = lambda x: hashlib.sha1(x.encode("ascii")).hexdigest()
        else:
            raise ValueError(
                "Unsupported digest authentication algorithm %r" % algorithm
            )
        KD = lambda s, d: H("%s:%s" % (s, d))
        return H, KD

    def get_entity_digest(self, data, chal):
        return None


class HTTPDigestAuthHandler(BaseHandler, AbstractDigestAuthHandler):
    auth_header = 'Authorization'
    handler_order = 490

    def http_error_401(self, req, fp, code, msg, headers):
        host = urllib.parse.urlparse(req.full_url)[1]
        retry = self.http_error_auth_reqed('www-authenticate', host, req, headers)
        self.reset_retry_count()
        return retry


class ProxyDigestAuthHandler(BaseHandler, AbstractDigestAuthHandler):
    auth_header = 'Proxy-Authorization'
    handler_order = 490

    def http_error_407(self, req, fp, code, msg, headers):
        host = req.host
        retry = self.http_error_auth_reqed('proxy-authenticate', host, req, headers)
        self.reset_retry_count()
        return retry


# ── HTTP / Unknown / File / Data Handlers ────────────────────────────────────

class AbstractHTTPHandler(BaseHandler):

    def __init__(self, debuglevel=None):
        import http.client

        self._debuglevel = (
            debuglevel
            if debuglevel is not None
            else http.client.HTTPConnection.debuglevel
        )

    def set_http_debuglevel(self, level):
        self._debuglevel = level

    def _get_content_length(self, request):
        import http.client

        return http.client.HTTPConnection._get_content_length(
            request.data,
            request.get_method(),
        )

    def do_request_(self, request):
        host = request.host
        if not host:
            raise urllib.error.URLError('no host given')

        if request.data is not None:
            data = request.data
            if isinstance(data, str):
                msg = (
                    "POST data should be bytes, an iterable of bytes, "
                    "or a file object. It cannot be of type str."
                )
                raise TypeError(msg)
            if not request.has_header('Content-type'):
                request.add_unredirected_header(
                    'Content-type', 'application/x-www-form-urlencoded'
                )
            if (
                not request.has_header('Content-length')
                and not request.has_header('Transfer-encoding')
            ):
                content_length = self._get_content_length(request)
                if content_length is not None:
                    request.add_unredirected_header(
                        'Content-length', str(content_length)
                    )
                else:
                    request.add_unredirected_header(
                        'Transfer-encoding', 'chunked'
                    )

        sel_host = host
        if request.has_proxy():
            scheme, sel = _splittype(request.selector)
            sel_host, sel_path = _splithost(sel)
        if not request.has_header('Host'):
            request.add_unredirected_header('Host', sel_host)
        for name, value in self.parent.addheaders:
            name = name.capitalize()
            if not request.has_header(name):
                request.add_unredirected_header(name, value)

        return request

    def do_open(self, http_class, req, **http_conn_args):
        """Return an HTTPResponse object for the request, using http_class."""
        host = req.host
        if not host:
            raise urllib.error.URLError('no host given')

        h = http_class(host, timeout=req.timeout, **http_conn_args)
        h.set_debuglevel(self._debuglevel)

        headers = dict(req.unredirected_hdrs)
        headers.update(
            {k: v for k, v in req.headers.items() if k not in headers}
        )

        headers["Connection"] = "close"
        headers = {name.title(): val for name, val in headers.items()}

        if req._tunnel_host:
            tunnel_headers = {}
            proxy_auth_hdr = "Proxy-Authorization"
            if proxy_auth_hdr in headers:
                tunnel_headers[proxy_auth_hdr] = headers[proxy_auth_hdr]
                del headers[proxy_auth_hdr]
            h.set_tunnel(req._tunnel_host, headers=tunnel_headers)

        try:
            try:
                h.request(
                    req.get_method(),
                    req.selector,
                    req.data,
                    headers,
                    encode_chunked=req.has_header('Transfer-encoding'),
                )
            except OSError as err:
                raise urllib.error.URLError(err)
            r = h.getresponse()
        except BaseException:
            h.close()
            raise

        if h.sock:
            h.sock.close()
            h.sock = None

        r.url = req.get_full_url()
        r.msg = r.reason
        return r


class HTTPHandler(AbstractHTTPHandler):

    def http_open(self, req):
        import http.client

        return self.do_open(http.client.HTTPConnection, req)

    http_request = AbstractHTTPHandler.do_request_


class UnknownHandler(BaseHandler):
    def unknown_open(self, req):
        type = req.type
        raise urllib.error.URLError('unknown url type: %s' % type)


def parse_keqv_list(l):
    """Parse list of key=value strings where keys are not duplicated."""
    parsed = {}
    for elt in l:
        k, v = elt.split('=', 1)
        if v[0] == '"' and v[-1] == '"':
            v = v[1:-1]
        parsed[k] = v
    return parsed


def parse_http_list(s):
    """Parse lists as described by RFC 2068 Section 2."""
    res = []
    part = ''

    escape = quote_flag = False
    for cur in s:
        if escape:
            part += cur
            escape = False
            continue
        if quote_flag:
            if cur == '\\':
                escape = True
                continue
            elif cur == '"':
                quote_flag = False
            part += cur
            continue

        if cur == ',':
            res.append(part)
            part = ''
            continue

        if cur == '"':
            quote_flag = True

        part += cur

    if part:
        res.append(part)

    return [part.strip() for part in res]


def _get_host_ips(name):
    if hasattr(socket, "gethostbyname_ex"):
        return list(socket.gethostbyname_ex(name)[2])
    ips = []
    for res in socket.getaddrinfo(name, None, socket.AF_INET):
        ip = res[4][0]
        if ip not in ips:
            ips.append(ip)
    if not ips:
        ips.append(socket.gethostbyname(name))
    return ips


class FileHandler(BaseHandler):
    def file_open(self, req):
        url = req.selector
        if (
            url[:2] == '//'
            and url[2:3] != '/'
            and (req.host and req.host != 'localhost')
        ):
            if req.host not in self.get_names():
                raise urllib.error.URLError(
                    "file:// scheme is supported only on localhost"
                )
        else:
            return self.open_local_file(req)

    names = None

    def get_names(self):
        if FileHandler.names is None:
            try:
                FileHandler.names = tuple(
                    _get_host_ips('localhost') + _get_host_ips(socket.gethostname())
                )
            except socket.gaierror:
                FileHandler.names = (socket.gethostbyname('localhost'),)
        return FileHandler.names

    def open_local_file(self, req):
        import email
        import email.utils
        import mimetypes

        host = req.host
        filename = req.selector
        localfile = url2pathname(filename)
        try:
            stats = os.stat(localfile)
            size = stats.st_size
            modified = email.utils.formatdate(stats.st_mtime, usegmt=True)
            mtype = mimetypes.guess_type(filename)[0]
            headers = email.message_from_string(
                'Content-type: %s\nContent-length: %d\nLast-modified: %s\n'
                % (mtype or 'text/plain', size, modified)
            )
            port = None
            if host:
                host, port = _splitport(host)
            if not host or (
                not port and _safe_gethostbyname(host) in self.get_names()
            ):
                if host:
                    origurl = 'file://' + host + filename
                else:
                    origurl = 'file://' + filename
                return urllib.response.addinfourl(
                    open(localfile, 'rb'), headers, origurl
                )
        except OSError as exp:
            raise urllib.error.URLError(exp)
        raise urllib.error.URLError('file not on local host')


def _safe_gethostbyname(host):
    try:
        return socket.gethostbyname(host)
    except socket.gaierror:
        return None


class DataHandler(BaseHandler):
    def data_open(self, req):
        import email

        # data URLs as specified in RFC 2397.
        url = req.full_url

        scheme, data = url.split(":", 1)
        mediatype, data = data.split(",", 1)

        # even base64 encoded data URLs might be quoted so unquote in any case:
        data = urllib.parse.unquote_to_bytes(data)
        if mediatype.endswith(";base64"):
            data = base64.decodebytes(data)
            mediatype = mediatype[:-7]

        if not mediatype:
            mediatype = "text/plain;charset=US-ASCII"

        headers = email.message_from_string(
            "Content-type: %s\nContent-length: %d\n" % (mediatype, len(data))
        )

        return urllib.response.addinfourl(io.BytesIO(data), headers, url)


# ── OS-specific pathname <-> file URL conversion ─────────────────────────────

if os.name == 'nt':
    def url2pathname(url):
        """OS-specific conversion from a relative URL of the 'file' scheme
        to a file system path; not recommended for general use."""
        url = url.replace(':', '|')
        if '|' not in url:
            if url[:4] == '////':
                url = url[2:]
            components = url.split('/')
            return urllib.parse.unquote('\\'.join(components))
        comp = url.split('|')
        if len(comp) != 2 or not comp[0] or comp[0][-1] not in string.ascii_letters:
            raise OSError('Bad URL: ' + url)
        drive = comp[0][-1].upper()
        components = comp[1].split('/')
        path = drive + ':'
        for c in components:
            if c:
                path = path + '\\' + urllib.parse.unquote(c)
        if path.endswith(':') and url.endswith('/'):
            path += '\\'
        return path

    def pathname2url(p):
        """OS-specific conversion from a file system path to a relative URL
        of the 'file' scheme; not recommended for general use."""
        if p[:4] == '\\\\?\\':
            p = p[4:]
            if p[:4].upper() == 'UNC\\':
                p = '\\' + p[4:]
            elif p[1:2] != ':':
                raise OSError('Bad path: ' + p)
        if ':' not in p:
            if p[:2] == '\\\\':
                p = '\\\\' + p
            components = p.split('\\')
            return urllib.parse.quote('/'.join(components))
        comp = p.split(':', maxsplit=2)
        if len(comp) != 2 or len(comp[0]) > 1:
            raise OSError('Bad path: ' + p)

        drive = urllib.parse.quote(comp[0].upper())
        components = comp[1].split('\\')
        path = '///' + drive + ':'
        for c in components:
            if c:
                path = path + '/' + urllib.parse.quote(c)
        return path
else:
    def url2pathname(pathname):
        """OS-specific conversion from a relative URL of the 'file' scheme
        to a file system path; not recommended for general use."""
        return urllib.parse.unquote(pathname)

    def pathname2url(pathname):
        """OS-specific conversion from a file system path to a relative URL
        of the 'file' scheme; not recommended for general use."""
        return urllib.parse.quote(pathname)


# ── Utility functions ────────────────────────────────────────────────────────

_localhost = None


def localhost():
    """Return the IP address of the magic hostname 'localhost'."""
    global _localhost
    if _localhost is None:
        _localhost = socket.gethostbyname('localhost')
    return _localhost


_thishost = None


def thishost():
    """Return the IP addresses of the current host."""
    global _thishost
    if _thishost is None:
        try:
            _thishost = tuple(_get_host_ips(socket.gethostname()))
        except socket.gaierror:
            _thishost = tuple(_get_host_ips('localhost'))
    return _thishost


_noheaders = None


def noheaders():
    """Return an empty email Message object."""
    import email

    global _noheaders
    if _noheaders is None:
        _noheaders = email.message_from_string("")
    return _noheaders


# ── Proxy handling ───────────────────────────────────────────────────────────

def getproxies_environment():
    """Return a dictionary of scheme -> proxy server URL mappings."""
    proxies = {}
    environment = []
    for name in os.environ.keys():
        if len(name) > 5 and name[-6] == "_" and name[-5:].lower() == "proxy":
            value = os.environ[name]
            proxy_name = name[:-6].lower()
            environment.append((name, value, proxy_name))
            if value:
                proxies[proxy_name] = value
    if 'REQUEST_METHOD' in os.environ:
        proxies.pop('http', None)
    for name, value, proxy_name in environment:
        if name[-6:] == '_proxy':
            if value:
                proxies[proxy_name] = value
            else:
                proxies.pop(proxy_name, None)
    return proxies


def proxy_bypass_environment(host, proxies=None):
    """Test if proxies should not be used for a particular host."""
    if proxies is None:
        proxies = getproxies_environment()
    try:
        no_proxy = proxies['no']
    except KeyError:
        return False
    if no_proxy == '*':
        return True
    host = host.lower()
    hostonly, port = _splitport(host)
    for name in no_proxy.split(','):
        name = name.strip()
        if name:
            name = name.lstrip('.')
            name = name.lower()
            if hostonly == name or host == name:
                return True
            name = '.' + name
            if hostonly.endswith(name) or host.endswith(name):
                return True
    return False


getproxies = getproxies_environment
proxy_bypass = proxy_bypass_environment
