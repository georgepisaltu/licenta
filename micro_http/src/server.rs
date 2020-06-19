use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::path::Path;

use common::Version;
pub use common::{ConnectionError, RequestError, ServerError};
use common::message::Message;
use common::net::{PollableListener, PollableStream};
use connection::HttpConnection;
use request::Request;
use response::{Response, StatusCode};
use std::collections::HashMap;

use common::epoll::{ControlOperation, Epoll, EPOLL_IN, EPOLL_OUT, EpollEvent, EventSet};

static SERVER_FULL_ERROR_MESSAGE: &[u8] = b"HTTP/1.1 503\r\n\
                                            Server: Firecracker API\r\n\
                                            Connection: close\r\n\
                                            Content-Length: 40\r\n\r\n{ \"error\": \"Too many open connections\" }";
const MAX_CONNECTIONS: usize = 10;

type Result<T> = std::result::Result<T, ServerError>;

/// Wrapper over `Request` which adds an identification token.
pub struct ServerRequest {
    /// Inner request.
    pub request: Request,
    /// Identification token.
    id: u64,
}

impl ServerRequest {
    /// Creates a new `ServerRequest` object from an existing `Request`,
    /// adding an identification token.
    pub fn new(request: Request, id: u64) -> Self {
        Self { request, id }
    }

    /// Returns a reference to the inner request.
    pub fn inner(&self) -> &Request {
        &self.request
    }

    /// Calls the function provided on the inner request to obtain the response.
    /// The response is then wrapped in a `ServerResponse`.
    ///
    /// Returns a `ServerResponse` ready for yielding to the server
    pub fn process<F>(&self, callable: F) -> ServerResponse
    where
        F: Fn(&Request) -> Response,
    {
        let http_response = callable(self.inner());
        ServerResponse::new(http_response, self.id)
    }
}

/// Wrapper over `Response` which adds an identification token.
pub struct ServerResponse {
    /// Inner response.
    response: Response,
    /// Identification token.
    id: u64,
}

impl ServerResponse {
    fn new(response: Response, id: u64) -> Self {
        Self { response, id }
    }
}

/// Describes the state of the connection as far as data exchange
/// on the stream is concerned.
#[derive(PartialOrd, PartialEq)]
enum ClientConnectionState {
    AwaitingIncoming,
    AwaitingOutgoing,
    Closed,
}

/// Wrapper over `HttpConnection` which keeps track of yielded
/// requests and absorbed responses.
struct ClientConnection<T> {
    /// The `HttpConnection` object which handles data exchange.
    connection: HttpConnection<T>,
    /// The state of the connection in the `epoll` structure.
    state: ClientConnectionState,
    /// Represents the difference between yielded requests and
    /// absorbed responses.
    /// This has to be `0` if we want to drop the connection.
    in_flight_response_count: u32,
}

impl<T: Read + Write> ClientConnection<T> {
    fn new(connection: HttpConnection<T>) -> Self {
        Self {
            connection,
            state: ClientConnectionState::AwaitingIncoming,
            in_flight_response_count: 0,
        }
    }

    fn read(&mut self) -> Result<Vec<Request>> {
        // Data came into the connection.
        let mut parsed_requests = vec![];
        match self.connection.try_read() {
            Err(ConnectionError::ConnectionClosed) => {
                // Connection timeout.
                self.state = ClientConnectionState::Closed;
                // We don't want to propagate this to the server and we will
                // return no requests and wait for the connection to become
                // safe to drop.
                return Ok(vec![]);
            }
            Err(ConnectionError::StreamError(inner)) => {
                // Reading from the connection failed.
                // We should try to write an error message regardless.
                let mut internal_error_response =
                    Response::new(Version::Http11, StatusCode::InternalServerError);
                internal_error_response.with_body(inner.to_string().as_bytes());
                self.connection.enqueue_response(internal_error_response);
            }
            Err(ConnectionError::ParseError(inner)) => {
                // An error occurred while parsing the read bytes.
                // Check if there are any valid parsed requests in the queue.
                while let Some(_discarded_request) = self.connection.pop_parsed_request() {}

                // Send an error response for the request that gave us the error.
                let mut error_response = Response::new(Version::Http11, StatusCode::BadRequest);
                error_response.with_body(format!(
                    "{{ \"error\": \"{}\nAll previous unanswered requests will be dropped.\" }}",
                    inner.to_string()
                ).as_bytes());
                self.connection.enqueue_response(error_response);
            }
            Err(ConnectionError::InvalidWrite) => {
                // This is unreachable because `HttpConnection::try_read()` cannot return this error variant.
                unreachable!();
            }
            Ok(()) => {
                while let Some(request) = self.connection.pop_parsed_request() {
                    // Add all valid requests to `parsed_requests`.
                    parsed_requests.push(request);
                }
            }
        }
        self.in_flight_response_count += parsed_requests.len() as u32;
        // If the state of the connection has changed, we need to update
        // the event set in the `epoll` structure.
        if self.connection.pending_write() {
            self.state = ClientConnectionState::AwaitingOutgoing;
        }

        Ok(parsed_requests)
    }

    fn write(&mut self) -> Result<()> {
        // The stream is available for writing.
        match self.connection.try_write() {
            Err(ConnectionError::ConnectionClosed) | Err(ConnectionError::StreamError(_)) => {
                // Writing to the stream failed so it will be removed.
                self.state = ClientConnectionState::Closed;
            }
            Err(ConnectionError::InvalidWrite) => {
                // A `try_write` call was performed on a connection that has nothing
                // to write.
                return Err(ServerError::ConnectionError(ConnectionError::InvalidWrite));
            }
            _ => {
                // Check if we still have bytes to write for this connection.
                if !self.connection.pending_write() {
                    self.state = ClientConnectionState::AwaitingIncoming;
                }
            }
        }
        Ok(())
    }

    fn enqueue_response(&mut self, response: Response) {
        if self.state != ClientConnectionState::Closed {
            self.connection.enqueue_response(response);
        }
        self.in_flight_response_count -= 1;
    }

    // Returns `true` if the connection is closed and safe to drop.
    fn is_done(&self) -> bool {
        self.state == ClientConnectionState::Closed
            && !self.connection.pending_write()
            && self.in_flight_response_count == 0
    }
}

/// HTTP Server implementation using Unix Domain Sockets and `EPOLL` to
/// handle multiple connections on the same thread.
///
/// The function that handles incoming connections, parses incoming
/// requests and sends responses for awaiting requests is `requests`.
/// It can be called in a loop, which will render the thread that the
/// server runs on incapable of performing other operations, or it can
/// be used in another `EPOLL` structure, as it provides its `epoll`,
/// which is a wrapper over the file descriptor of the epoll structure
/// used within the server, and it can be added to another one using
/// the `EPOLLIN` flag. Whenever there is a notification on that fd,
/// `requests` should be called once.
///
/// # Example
///
/// ## Starting and running the server
///
/// ```
/// use micro_http::{HttpServer, Response, StatusCode};
///
/// let path_to_socket = "/tmp/example.sock";
/// std::fs::remove_file(path_to_socket).unwrap_or_default();
///
/// // Start the server.
/// let mut server = HttpServer::new_uds(path_to_socket).unwrap();
/// server.start_server().unwrap();
///
/// // Connect a client to the server so it doesn't block in our example.
/// let mut socket = std::os::unix::net::UnixStream::connect(path_to_socket).unwrap();
///
/// // Server loop processing requests.
/// loop {
///     for request in server.requests().unwrap() {
///         let response = request.process(|request| {
///             // Your code here.
///             Response::new(request.http_version(), StatusCode::NoContent)
///         });
///         server.respond(response);
///     }
///     // Break this example loop.
///     break;
/// }
/// ```
pub struct HttpServer {
    /// Socket on which we listen for new connections.
    socket: PollableListener,
    /// Server's epoll instance.
    epoll: Epoll,
    /// Holds the token-connection pairs of the server.
    /// Each connection has an associated identification token, which is
    /// the file descriptor of the underlying stream.
    /// We use the file descriptor of the stream as the key for mapping
    /// connections because the 1-to-1 relation is guaranteed by the OS.
    connections: HashMap<RawFd, ClientConnection<PollableStream>>,
}

impl HttpServer {
    /// Constructor for `HttpServer` on a TCP socket.
    ///
    /// Returns the newly formed `HttpServer`.
    ///
    /// # Errors
    /// Returns an `IOError` when binding or `epoll::create` fails.
    pub fn new_tcp<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let socket = PollableListener::bind_tcp(addr).map_err(ServerError::IOError)?;
        let epoll = Epoll::new().map_err(ServerError::IOError)?;
        Ok(Self {
            socket,
            epoll,
            connections: HashMap::new(),
        })
    }

    /// Constructor for `HttpServer` on a Unix Domain Socket.
    ///
    /// Returns the newly formed `HttpServer`.
    ///
    /// # Errors
    /// Returns an `IOError` when binding or `epoll::create` fails.
    pub fn new_uds<P: AsRef<Path>>(path_to_socket: P) -> Result<Self> {
        let socket = PollableListener::bind_uds(path_to_socket).map_err(ServerError::IOError)?;
        let epoll = Epoll::new().map_err(ServerError::IOError)?;
        Ok(Self {
            socket,
            epoll,
            connections: HashMap::new(),
        })
    }

    /// Starts the HTTP Server.
    pub fn start_server(&mut self) -> Result<()> {
        // Add the socket on which we listen for new connections to the
        // `epoll` structure.
        Self::epoll_add(&self.epoll, self.socket.as_raw_fd())
    }

    pub fn requests(&mut self) -> Result<Vec<ServerRequest>> {
        let mut parsed_requests: Vec<ServerRequest> = vec![];
        let mut events = vec![EpollEvent::default(); MAX_CONNECTIONS];
        // This is a wrapper over the syscall `epoll_wait` and it will block the
        // current thread until at least one event is received.
        // The received notifications will then populate the `events` array with
        // `event_count` elements, where 1 <= event_count <= MAX_CONNECTIONS.
        let event_count = match self.epoll.wait(MAX_CONNECTIONS, &mut events[..]) {
            Ok(event_count) => event_count,
            Err(e) if e.raw_os_error() == Some(libc::EINTR) => 0,
            Err(e) => return Err(ServerError::IOError(e)),
        };
        // We use `take()` on the iterator over `events` as, even though only
        // `events_count` events have been inserted into `events`, the size of
        // the array is still `MAX_CONNECTIONS`, so we discard empty elements
        // at the end of the array.
        for e in events.iter().take(event_count) {
            // Check the file descriptor which produced the notification `e`.
            // It could be that we have a new connection, or one of our open
            // connections is ready to exchange data with a client.
            if e.fd() == self.socket.as_raw_fd() {
                // We have received a notification on the listener socket, which
                // means we have a new connection to accept.
                match self.handle_new_connection() {
                    // If the server is full, we send a message to the client
                    // notifying them that we will close the connection, then
                    // we discard it.
                    Err(ServerError::ServerFull) => {
                        self.socket
                            .accept()
                            .map_err(ServerError::IOError)
                            .and_then(move |mut stream| {
                                stream
                                    .write(SERVER_FULL_ERROR_MESSAGE)
                                    .map_err(ServerError::IOError)
                            })?;
                    }
                    // An internal error will compromise any in-flight requests.
                    Err(error) => return Err(error),
                    Ok(()) => {}
                };
            } else {
                // We have a notification on one of our open connections.
                let fd = e.fd();
                let client_connection = self.connections.get_mut(&fd).unwrap();
                if e.event_set().contains(EPOLL_IN) {
                    // We have bytes to read from this connection.
                    // If our `read` yields `Request` objects, we wrap them with an ID before
                    // handing them to the user.
                    parsed_requests.append(
                        &mut client_connection
                            .read()?
                            .into_iter()
                            .map(|request| ServerRequest::new(request, e.data()))
                            .collect(),
                    );
                    // If the connection was incoming before we read and we now have to write
                    // either an error message or an `expect` response, we change its `epoll`
                    // event set to notify us when the stream is ready for writing.
                    if client_connection.state == ClientConnectionState::AwaitingOutgoing {
                        Self::epoll_mod(&self.epoll, fd, EventSet::new(EPOLL_OUT))?;
                    }
                } else if e.event_set().contains(EPOLL_OUT) {
                    // We have bytes to write on this connection.
                    client_connection.write()?;
                    // If the connection was outgoing before we tried to write the responses
                    // and we don't have any more responses to write, we change the `epoll`
                    // event set to notify us when we have bytes to read from the stream.
                    if client_connection.state == ClientConnectionState::AwaitingIncoming {
                        Self::epoll_mod(&self.epoll, fd, EventSet::new(EPOLL_IN))?;
                    }
                }
            }
        }

        // Remove dead connections.
        self.connections
            .retain(|_, client_connection| !client_connection.is_done());

        Ok(parsed_requests)
    }

    pub fn epoll(&self) -> &Epoll {
        &self.epoll
    }

    /// Enqueues the provided responses in the outgoing connection.
    ///
    /// # Errors
    /// `IOError` is returned when an `epoll::ctl` operation fails.
    pub fn enqueue_responses(&mut self, responses: Vec<ServerResponse>) -> Result<()> {
        for response in responses {
            self.respond(response)?;
        }

        Ok(())
    }

    /// Adds the provided response to the outgoing buffer in the corresponding connection.
    ///
    /// # Errors
    /// `IOError` is returned when an `epoll::ctl` operation fails.
    pub fn respond(&mut self, response: ServerResponse) -> Result<()> {
        if let Some(client_connection) = self.connections.get_mut(&(response.id as i32)) {
            // If the connection was incoming before we enqueue the response, we change its
            // `epoll` event set to notify us when the stream is ready for writing.
            if let ClientConnectionState::AwaitingIncoming = client_connection.state {
                client_connection.state = ClientConnectionState::AwaitingOutgoing;
                Self::epoll_mod(&self.epoll, response.id as RawFd, EventSet::new(EPOLL_OUT))?;
            }
            client_connection.enqueue_response(response.response);
        }
        Ok(())
    }

    /// Accepts a new incoming connection and adds it to the `epoll` notification structure.
    ///
    /// # Errors
    /// `IOError` is returned when socket or epoll operations fail.
    /// `ServerFull` is returned if server full capacity has been reached.
    fn handle_new_connection(&mut self) -> Result<()> {
        if self.connections.len() == MAX_CONNECTIONS {
            // If we want a replacement policy for connections
            // this is where we will have it.
            return Err(ServerError::ServerFull);
        }

        self.socket
            .accept()
            .map_err(ServerError::IOError)
            .and_then(|stream| {
                // `HttpConnection` is supposed to work with non-blocking streams.
                stream
                    .set_nonblocking(true)
                    .map(|_| stream)
                    .map_err(ServerError::IOError)
            })
            .and_then(|stream| {
                // Add the stream to the `epoll` structure and listen for bytes to be read.
                Self::epoll_add(&self.epoll, stream.as_raw_fd())?;
                // Then add it to our open connections.
                self.connections.insert(
                    stream.as_raw_fd(),
                    ClientConnection::new(HttpConnection::new(stream)),
                );
                Ok(())
            })
    }

    /// Changes the event type for a connection to either listen for incoming bytes
    /// or for when the stream is ready for writing.
    ///
    /// # Errors
    /// `IOError` is returned when an `EPOLL_CTL_MOD` control operation fails.
    fn epoll_mod(epoll: &Epoll, stream_fd: RawFd, evset: EventSet) -> Result<()> {
        let event = EpollEvent::new(evset, stream_fd as u64);
        epoll
            .ctl(ControlOperation::Modify, stream_fd, &event)
            .map_err(ServerError::IOError)
    }

    /// Adds a stream to the `epoll` notification structure with the `EPOLLIN` event set.
    ///
    /// # Errors
    /// `IOError` is returned when an `EPOLL_CTL_ADD` control operation fails.
    fn epoll_add(epoll: &Epoll, stream_fd: RawFd) -> Result<()> {
        epoll
            .ctl(
                ControlOperation::Add,
                stream_fd,
                &EpollEvent::new(EventSet::new(EPOLL_IN), stream_fd as u64),
            )
            .map_err(ServerError::IOError)
    }
}

#[cfg(test)]
mod tests {
    extern crate vmm_sys_util;

    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    use server::tests::vmm_sys_util::tempfile::TempFile;

    fn get_temp_socket_file() -> TempFile {
        let mut path_to_socket = TempFile::new().unwrap();
        path_to_socket.remove().unwrap();
        path_to_socket
    }

    impl PartialEq for Request {
        fn eq(&self, other: &Self) -> bool {
            // Ignore the other fields of Request for now because they are not used.
            self.request_line == other.request_line
                && self.headers.content_length() == other.headers.content_length()
                && self.headers.map == other.headers.map
        }
    }

    #[test]
    fn test_wait_one_connection() {
        let path_to_socket = get_temp_socket_file();

        let mut server = HttpServer::new_uds(path_to_socket.as_path()).unwrap();
        server.start_server().unwrap();

        // Test one incoming connection.
        let mut socket = UnixStream::connect(path_to_socket.as_path()).unwrap();
        assert!(server.requests().unwrap().is_empty());

        socket
            .write_all(
                b"PATCH /machine-config HTTP/1.1\r\n\
                         Content-Length: 13\r\n\
                         Content-Type: application/json\r\n\r\nwhatever body",
            )
            .unwrap();

        let mut req_vec = server.requests().unwrap();
        let server_request = req_vec.remove(0);

        server
            .respond(server_request.process(|_request| {
                let mut response = Response::new(Version::Http11, StatusCode::OK);
                let response_body = b"response body";
                response.with_body(response_body);
                response
            }))
            .unwrap();
        assert!(server.requests().unwrap().is_empty());

        let mut buf: [u8; 1024] = [0; 1024];
        assert!(socket.read(&mut buf[..]).unwrap() > 0);
    }

    #[test]
    fn test_wait_concurrent_connections() {
        let path_to_socket = get_temp_socket_file();

        let mut server = HttpServer::new_uds(path_to_socket.as_path()).unwrap();
        server.start_server().unwrap();

        // Test two concurrent connections.
        let mut first_socket = UnixStream::connect(path_to_socket.as_path()).unwrap();
        assert!(server.requests().unwrap().is_empty());

        first_socket
            .write_all(
                b"PATCH /machine-config HTTP/1.1\r\n\
                               Content-Length: 13\r\n\
                               Content-Type: application/json\r\n\r\nwhatever body",
            )
            .unwrap();
        let mut second_socket = UnixStream::connect(path_to_socket.as_path()).unwrap();

        let mut req_vec = server.requests().unwrap();
        let server_request = req_vec.remove(0);

        server
            .respond(server_request.process(|_request| {
                let mut response = Response::new(Version::Http11, StatusCode::OK);
                let response_body = b"response body";
                response.with_body(response_body);
                response
            }))
            .unwrap();
        second_socket
            .write_all(
                b"GET /machine-config HTTP/1.1\r\n\
                                Content-Length: 20\r\n\
                                Content-Type: application/json\r\n\r\nwhatever second body",
            )
            .unwrap();

        let mut req_vec = server.requests().unwrap();
        let second_server_request = req_vec.remove(0);

        assert_eq!(
            second_server_request.request,
            Request::try_from(
                b"GET /machine-config HTTP/1.1\r\n\
            Content-Length: 20\r\n\
            Content-Type: application/json\r\n\r\nwhatever second body"
            )
            .unwrap()
        );

        let mut buf: [u8; 1024] = [0; 1024];
        assert!(first_socket.read(&mut buf[..]).unwrap() > 0);
        first_socket.shutdown(std::net::Shutdown::Both).unwrap();

        server
            .respond(second_server_request.process(|_request| {
                let mut response = Response::new(Version::Http11, StatusCode::OK);
                let response_body = b"response second body";
                response.with_body(response_body);
                response
            }))
            .unwrap();

        assert!(server.requests().unwrap().is_empty());
        let mut buf: [u8; 1024] = [0; 1024];
        assert!(second_socket.read(&mut buf[..]).unwrap() > 0);
        second_socket.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(server.requests().unwrap().is_empty());
    }

    #[test]
    fn test_wait_expect_connection() {
        let path_to_socket = get_temp_socket_file();

        let mut server = HttpServer::new_uds(path_to_socket.as_path()).unwrap();
        server.start_server().unwrap();

        // Test one incoming connection with `Expect: 100-continue`.
        let mut socket = UnixStream::connect(path_to_socket.as_path()).unwrap();
        assert!(server.requests().unwrap().is_empty());

        socket
            .write_all(
                b"PATCH /machine-config HTTP/1.1\r\n\
                         Content-Length: 13\r\n\
                         Expect: 100-continue\r\n\r\n",
            )
            .unwrap();
        // `wait` on server to receive what the client set on the socket.
        // This will set the stream direction to `Outgoing`, as we need to send a `100 CONTINUE` response.
        let req_vec = server.requests().unwrap();
        assert!(req_vec.is_empty());
        // Another `wait`, this time to send the response.
        // Will be called because of an `EPOLLOUT` notification.
        let req_vec = server.requests().unwrap();
        assert!(req_vec.is_empty());
        let mut buf: [u8; 1024] = [0; 1024];
        assert!(socket.read(&mut buf[..]).unwrap() > 0);

        socket.write_all(b"whatever body").unwrap();
        let mut req_vec = server.requests().unwrap();
        let server_request = req_vec.remove(0);

        server
            .respond(server_request.process(|_request| {
                let mut response = Response::new(Version::Http11, StatusCode::OK);
                let response_body = b"response body";
                response.with_body(response_body);
                response
            }))
            .unwrap();

        let req_vec = server.requests().unwrap();
        assert!(req_vec.is_empty());

        let mut buf: [u8; 1024] = [0; 1024];
        assert!(socket.read(&mut buf[..]).unwrap() > 0);
    }

    #[test]
    fn test_wait_many_connections() {
        let path_to_socket = get_temp_socket_file();

        let mut server = HttpServer::new_uds(path_to_socket.as_path()).unwrap();
        server.start_server().unwrap();

        let mut sockets: Vec<UnixStream> = Vec::with_capacity(11);
        for _ in 0..MAX_CONNECTIONS {
            sockets.push(UnixStream::connect(path_to_socket.as_path()).unwrap());
            assert!(server.requests().unwrap().is_empty());
        }

        sockets.push(UnixStream::connect(path_to_socket.as_path()).unwrap());
        assert!(server.requests().unwrap().is_empty());
        let mut buf: [u8; 120] = [0; 120];
        sockets[MAX_CONNECTIONS].read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], SERVER_FULL_ERROR_MESSAGE);
    }

    #[test]
    fn test_wait_parse_error() {
        let path_to_socket = get_temp_socket_file();

        let mut server = HttpServer::new_uds(path_to_socket.as_path()).unwrap();
        server.start_server().unwrap();

        // Test one incoming connection.
        let mut socket = UnixStream::connect(path_to_socket.as_path()).unwrap();
        socket.set_nonblocking(true).unwrap();
        assert!(server.requests().unwrap().is_empty());

        socket
            .write_all(
                b"PATCH /machine-config HTTP/1.1\r\n\
                         Content-Length: alpha\r\n\
                         Content-Type: application/json\r\n\r\nwhatever body",
            )
            .unwrap();

        assert!(server.requests().unwrap().is_empty());
        assert!(server.requests().unwrap().is_empty());
        let mut buf: [u8; 116] = [0; 116];
        assert!(socket.read(&mut buf[..]).unwrap() > 0);
        let error_message = b"HTTP/1.1 400\r\n\
                              Content-Length: 80\r\n\r\n{ \"error\": \"Invalid header.\n\
                              All previous unanswered requests will be dropped.\" }";
        assert_eq!(&buf[..], &error_message[..]);
    }

    #[test]
    fn test_wait_in_flight_responses() {
        let path_to_socket = get_temp_socket_file();

        let mut server = HttpServer::new_uds(path_to_socket.as_path()).unwrap();
        server.start_server().unwrap();

        // Test a connection dropped and then a new one appearing
        // before the user had a chance to send the response to the
        // first one.
        let mut first_socket = UnixStream::connect(path_to_socket.as_path()).unwrap();
        assert!(server.requests().unwrap().is_empty());

        first_socket
            .write_all(
                b"PATCH /machine-config HTTP/1.1\r\n\
                               Content-Length: 13\r\n\
                               Content-Type: application/json\r\n\r\nwhatever body",
            )
            .unwrap();

        let mut req_vec = server.requests().unwrap();
        let server_request = req_vec.remove(0);

        first_socket.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(server.requests().unwrap().is_empty());
        let mut second_socket = UnixStream::connect(path_to_socket.as_path()).unwrap();
        second_socket.set_nonblocking(true).unwrap();
        assert!(server.requests().unwrap().is_empty());

        server
            .enqueue_responses(vec![server_request.process(|_request| {
                let mut response = Response::new(Version::Http11, StatusCode::OK);
                let response_body = b"response body";
                response.with_body(response_body);
                response
            })])
            .unwrap();
        assert!(server.requests().unwrap().is_empty());
        assert_eq!(server.connections.len(), 1);
        let mut buf: [u8; 1024] = [0; 1024];
        assert!(second_socket.read(&mut buf[..]).is_err());

        second_socket
            .write_all(
                b"GET /machine-config HTTP/1.1\r\n\
                                Content-Length: 20\r\n\
                                Content-Type: application/json\r\n\r\nwhatever second body",
            )
            .unwrap();

        let mut req_vec = server.requests().unwrap();
        let second_server_request = req_vec.remove(0);

        assert_eq!(
            second_server_request.request,
            Request::try_from(
                b"GET /machine-config HTTP/1.1\r\n\
            Content-Length: 20\r\n\
            Content-Type: application/json\r\n\r\nwhatever second body"
            )
            .unwrap()
        );

        server
            .respond(second_server_request.process(|_request| {
                let mut response = Response::new(Version::Http11, StatusCode::OK);
                let response_body = b"response second body";
                response.with_body(response_body);
                response
            }))
            .unwrap();

        assert!(server.requests().unwrap().is_empty());
        let mut buf: [u8; 1024] = [0; 1024];
        assert!(second_socket.read(&mut buf[..]).unwrap() > 0);
        second_socket.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(server.requests().is_ok());
    }
}
