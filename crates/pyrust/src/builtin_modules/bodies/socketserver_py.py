"""Generic socket server classes."""

__version__ = "0.4"

import selectors
import socket
import sys
import threading
import traceback
from io import BufferedIOBase, BytesIO
from time import monotonic as time

__all__ = [
    "BaseServer",
    "TCPServer",
    "UDPServer",
    "ThreadingUDPServer",
    "ThreadingTCPServer",
    "BaseRequestHandler",
    "StreamRequestHandler",
    "DatagramRequestHandler",
    "ThreadingMixIn",
]

if hasattr(selectors, 'PollSelector'):
    _ServerSelector = selectors.PollSelector
else:
    _ServerSelector = selectors.SelectSelector


class BaseServer:
    """Base class for server classes."""

    timeout = None

    def __init__(self, server_address, RequestHandlerClass):
        """Constructor.  May be extended, do not override."""
        self.server_address = server_address
        self.RequestHandlerClass = RequestHandlerClass
        self.__is_shut_down = threading.Event()
        self.__shutdown_request = False

    def server_activate(self):
        """Called by constructor to activate the server."""
        pass

    def serve_forever(self, poll_interval=0.5):
        """Handle one request at a time until shutdown."""
        self.__is_shut_down.clear()
        try:
            with _ServerSelector() as selector:
                selector.register(self, selectors.EVENT_READ)

                while not self.__shutdown_request:
                    ready = selector.select(poll_interval)
                    if self.__shutdown_request:
                        break
                    if ready:
                        self._handle_request_noblock()

                    self.service_actions()
        finally:
            self.__shutdown_request = False
            self.__is_shut_down.set()

    def shutdown(self):
        """Stops the serve_forever loop."""
        self.__shutdown_request = True
        self.__is_shut_down.wait()

    def service_actions(self):
        """Called by the serve_forever() loop."""
        pass

    def handle_request(self):
        """Handle one request, possibly blocking. Respects self.timeout."""
        timeout = self.socket.gettimeout()
        if timeout is None:
            timeout = self.timeout
        elif self.timeout is not None:
            timeout = min(timeout, self.timeout)
        if timeout is not None:
            deadline = time() + timeout

        with _ServerSelector() as selector:
            selector.register(self, selectors.EVENT_READ)

            while True:
                if selector.select(timeout):
                    return self._handle_request_noblock()
                else:
                    if timeout is not None:
                        timeout = deadline - time()
                        if timeout < 0:
                            return self.handle_timeout()

    def _handle_request_noblock(self):
        """Handle one request, without blocking."""
        try:
            request, client_address = self.get_request()
        except OSError:
            return
        if self.verify_request(request, client_address):
            try:
                self.process_request(request, client_address)
            except Exception:
                self.handle_error(request, client_address)
                self.shutdown_request(request)
            except BaseException:
                self.shutdown_request(request)
                raise
        else:
            self.shutdown_request(request)

    def handle_timeout(self):
        """Called if no new request arrives within self.timeout."""
        pass

    def verify_request(self, request, client_address):
        """Verify the request. Return True if we should proceed."""
        return True

    def process_request(self, request, client_address):
        """Call finish_request."""
        self.finish_request(request, client_address)
        self.shutdown_request(request)

    def server_close(self):
        """Called to clean-up the server."""
        pass

    def finish_request(self, request, client_address):
        """Finish one request by instantiating RequestHandlerClass."""
        self.RequestHandlerClass(request, client_address, self)

    def shutdown_request(self, request):
        """Called to shutdown and close an individual request."""
        self.close_request(request)

    def close_request(self, request):
        """Called to clean up an individual request."""
        pass

    def handle_error(self, request, client_address):
        """Handle an error gracefully."""
        print('-' * 40, file=sys.stderr)
        print(
            'Exception occurred during processing of request from',
            client_address,
            file=sys.stderr,
        )
        traceback.print_exc()
        print('-' * 40, file=sys.stderr)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.server_close()


class TCPServer(BaseServer):
    """Base class for various socket-based server classes (default TCP)."""

    address_family = socket.AF_INET

    socket_type = socket.SOCK_STREAM

    request_queue_size = 5

    allow_reuse_address = False

    allow_reuse_port = False

    def __init__(self, server_address, RequestHandlerClass, bind_and_activate=True):
        """Constructor.  May be extended, do not override."""
        BaseServer.__init__(self, server_address, RequestHandlerClass)
        self.socket = socket.socket(self.address_family, self.socket_type)
        if bind_and_activate:
            try:
                self.server_bind()
                self.server_activate()
            except BaseException:
                self.server_close()
                raise

    def server_bind(self):
        """Called by constructor to bind the socket."""
        if self.allow_reuse_address and hasattr(socket, "SO_REUSEADDR"):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if self.allow_reuse_port and hasattr(socket, "SO_REUSEPORT"):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()

    def server_activate(self):
        """Called by constructor to activate the server."""
        self.socket.listen(self.request_queue_size)

    def server_close(self):
        """Called to clean-up the server."""
        self.socket.close()

    def fileno(self):
        """Return socket file number. Interface required by selector."""
        return self.socket.fileno()

    def get_request(self):
        """Get the request and client address from the socket."""
        return self.socket.accept()

    def shutdown_request(self, request):
        """Called to shutdown and close an individual request."""
        try:
            request.shutdown(socket.SHUT_WR)
        except OSError:
            pass
        self.close_request(request)

    def close_request(self, request):
        """Called to clean up an individual request."""
        request.close()


class UDPServer(TCPServer):
    """UDP server class."""

    allow_reuse_address = False

    allow_reuse_port = False

    socket_type = socket.SOCK_DGRAM

    max_packet_size = 8192

    def get_request(self):
        data, client_addr = self.socket.recvfrom(self.max_packet_size)
        return (data, self.socket), client_addr

    def server_activate(self):
        # No need to call listen() for UDP.
        pass

    def shutdown_request(self, request):
        # No need to shutdown anything.
        self.close_request(request)

    def close_request(self, request):
        # No need to close anything.
        pass


class _Threads(list):
    """Joinable list of all non-daemon threads."""

    def append(self, thread):
        self.reap()
        if thread.daemon:
            return
        super().append(thread)

    def pop_all(self):
        self[:], result = [], self[:]
        return result

    def join(self):
        for thread in self.pop_all():
            thread.join()

    def reap(self):
        self[:] = (thread for thread in self if thread.is_alive())


class _NoThreads:
    """Degenerate version of _Threads."""

    def append(self, thread):
        pass

    def join(self):
        pass


class ThreadingMixIn:
    """Mix-in class to handle each request in a new thread."""

    # Decides how threads will act upon termination of the main process.
    daemon_threads = False
    # If true, server_close() waits until all non-daemonic threads terminate.
    block_on_close = True
    # Used by server_close() to wait for all threads completion.  Each server
    # gets its own `_Threads` in `process_request()`; this class-level
    # placeholder only ever sees `append()`/`join()` no-ops.
    _threads = _NoThreads()

    def process_request_thread(self, request, client_address):
        """Same as in BaseServer but as a thread, with exception handling."""
        try:
            self.finish_request(request, client_address)
        except Exception:
            self.handle_error(request, client_address)
        finally:
            self.shutdown_request(request)

    def process_request(self, request, client_address):
        """Start a new thread to process the request."""
        if self.block_on_close:
            vars(self).setdefault('_threads', _Threads())
        t = threading.Thread(
            target=self.process_request_thread,
            args=(request, client_address),
        )
        t.daemon = self.daemon_threads
        self._threads.append(t)
        t.start()

    def server_close(self):
        super().server_close()
        self._threads.join()


class ThreadingUDPServer(ThreadingMixIn, UDPServer):
    pass


class ThreadingTCPServer(ThreadingMixIn, TCPServer):
    pass


class BaseRequestHandler:
    """Base class for request handler classes."""

    def __init__(self, request, client_address, server):
        self.request = request
        self.client_address = client_address
        self.server = server
        self.setup()
        try:
            self.handle()
        finally:
            self.finish()

    def setup(self):
        pass

    def handle(self):
        pass

    def finish(self):
        pass


class _SocketWriter(BufferedIOBase):
    """Simple writable BufferedIOBase implementation for a socket."""

    def __init__(self, sock):
        self._sock = sock
        self._closed = False

    @property
    def closed(self):
        return self._closed

    def readable(self):
        return False

    def writable(self):
        return True

    def seekable(self):
        return False

    def write(self, b):
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        self._sock.sendall(b)
        with memoryview(b) as view:
            return view.nbytes

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def close(self):
        self._closed = True

    def fileno(self):
        return self._sock.fileno()

    def __enter__(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        return self

    def __exit__(self, *args):
        self.close()


class StreamRequestHandler(BaseRequestHandler):
    """Define self.rfile and self.wfile for stream sockets."""

    rbufsize = -1
    wbufsize = 0

    timeout = None

    disable_nagle_algorithm = False

    def setup(self):
        self.connection = self.request
        if self.timeout is not None:
            self.connection.settimeout(self.timeout)
        if self.disable_nagle_algorithm:
            self.connection.setsockopt(
                socket.IPPROTO_TCP, socket.TCP_NODELAY, True
            )
        self.rfile = self.connection.makefile('rb', self.rbufsize)
        if self.wbufsize == 0:
            self.wfile = _SocketWriter(self.connection)
        else:
            self.wfile = self.connection.makefile('wb', self.wbufsize)

    def finish(self):
        if not self.wfile.closed:
            try:
                self.wfile.flush()
            except socket.error:
                pass
        self.wfile.close()
        self.rfile.close()


class DatagramRequestHandler(BaseRequestHandler):
    """Define self.rfile and self.wfile for datagram sockets."""

    def setup(self):
        self.packet, self.socket = self.request
        self.rfile = BytesIO(self.packet)
        self.wfile = BytesIO()

    def finish(self):
        self.socket.sendto(self.wfile.getvalue(), self.client_address)
