import sys
import http
import http.client
import http.server
import http.cookies
from enum import IntEnum, StrEnum
from http import HTTPStatus, HTTPMethod

# 1. Package and submodule identity
assert http.client is sys.modules["http.client"]
assert http.server is sys.modules["http.server"]
assert http.cookies is sys.modules["http.cookies"]
assert http.__all__ == ["HTTPStatus", "HTTPMethod"]
print("imports_ok")

# 2. HTTPStatus enum contract
assert issubclass(HTTPStatus, IntEnum)
assert issubclass(HTTPStatus, int)
assert len(HTTPStatus) == 62

assert HTTPStatus.OK == 200
assert HTTPStatus.OK.value == 200
assert HTTPStatus.OK.name == "OK"
assert HTTPStatus.OK.phrase == "OK"
assert HTTPStatus.OK.description == "Request fulfilled, document follows"
assert str(HTTPStatus.OK) == "200"
assert repr(HTTPStatus.OK) == "<HTTPStatus.OK: 200>"
assert HTTPStatus(200) is HTTPStatus.OK
assert HTTPStatus["NOT_FOUND"] is HTTPStatus.NOT_FOUND
assert HTTPStatus.NOT_FOUND == 404
assert HTTPStatus.NOT_FOUND.phrase == "Not Found"
assert HTTPStatus.IM_A_TEAPOT == 418
assert HTTPStatus.IM_A_TEAPOT.phrase == "I'm a Teapot"
assert HTTPStatus.IM_A_TEAPOT.description == "Server refuses to brew coffee because it is a teapot."
assert HTTPStatus.INTERNAL_SERVER_ERROR == 500
assert HTTPStatus.NETWORK_AUTHENTICATION_REQUIRED == 511
assert HTTPStatus.CONTINUE == 100
assert HTTPStatus.EARLY_HINTS == 103
assert HTTPStatus.PERMANENT_REDIRECT == 308
assert HTTPStatus.UNAVAILABLE_FOR_LEGAL_REASONS == 451

# Verify arithmetic / int compatibility
assert HTTPStatus.OK + 1 == 201
assert HTTPStatus.CREATED > HTTPStatus.OK

# Invalid status lookup raises ValueError
try:
    HTTPStatus(999)
    assert False, "expected ValueError"
except ValueError:
    pass

# Verify every single status code's phrase, description, and 3.12 category properties
for st in HTTPStatus:
    code = int(st)
    assert isinstance(st.phrase, str) and len(st.phrase) > 0
    assert isinstance(st.description, str)
    assert st.is_informational is (100 <= code <= 199)
    assert st.is_success is (200 <= code <= 299)
    assert st.is_redirection is (300 <= code <= 399)
    assert st.is_client_error is (400 <= code <= 499)
    assert st.is_server_error is (500 <= code <= 599)
    # Exactly one category flag is True
    flags = [
        st.is_informational,
        st.is_success,
        st.is_redirection,
        st.is_client_error,
        st.is_server_error,
    ]
    assert sum(1 for f in flags if f) == 1

print("status_summary", [(s.name, int(s), s.phrase, s.description) for s in HTTPStatus])

# 3. HTTPMethod enum contract
assert issubclass(HTTPMethod, StrEnum)
assert issubclass(HTTPMethod, str)
assert len(HTTPMethod) == 9

expected_methods = [
    "CONNECT", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE"
]
assert [m.name for m in HTTPMethod] == expected_methods
assert [m.value for m in HTTPMethod] == expected_methods
assert HTTPMethod.GET == "GET"
assert str(HTTPMethod.GET) == "GET"
assert repr(HTTPMethod.GET) == "<HTTPMethod.GET>"
assert HTTPMethod("POST") is HTTPMethod.POST
assert HTTPMethod["DELETE"] is HTTPMethod.DELETE

try:
    HTTPMethod("BREW")
    assert False, "expected ValueError"
except ValueError:
    pass

print("methods_summary", [(m.name, m.value, m.description) for m in HTTPMethod])
print("http_ok")
