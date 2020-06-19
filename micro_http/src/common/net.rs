use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

pub enum PollableListener {
    Tcp(TcpListener),
    Uds(UnixListener),
}

impl PollableListener {
    pub fn bind_tcp<A: ToSocketAddrs>(addr: A) -> std::result::Result<PollableListener, std::io::Error> {
        Ok(Self::Tcp(TcpListener::bind(addr)?))
    }

    pub fn bind_uds<P: AsRef<Path>>(path: P) -> std::result::Result<PollableListener, std::io::Error> {
        Ok(Self::Uds(UnixListener::bind(path)?))
    }

    pub fn accept(&self) -> std::result::Result<PollableStream, std::io::Error> {
        match self {
            Self::Tcp(listener) => listener.accept().and_then(move |(stream, _)| {
                Ok(PollableStream::Tcp(stream))
            }),
            Self::Uds(listener) => listener.accept().and_then(move |(stream, _)| {
                Ok(PollableStream::Uds(stream))
            }),
        }
    }
}

impl AsRawFd for PollableListener {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Tcp(listener) => listener.as_raw_fd(),
            Self::Uds(listener) => listener.as_raw_fd(),
        }
    }
}



pub enum PollableStream {
    Tcp(TcpStream),
    Uds(UnixStream),
}

impl Read for PollableStream {
    fn read(&mut self, buf: &mut [u8]) -> std::result::Result<usize, std::io::Error> {
        match self {
            Self::Tcp(stream) => stream.read(buf),
            Self::Uds(stream) => stream.read(buf),
        }
    }
}

impl Write for PollableStream {
    fn write(&mut self, buf: &[u8]) -> std::result::Result<usize, std::io::Error> {
        match self {
            Self::Tcp(stream) => stream.write(buf),
            Self::Uds(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::result::Result<(), std::io::Error> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            Self::Uds(stream) => stream.flush(),
        }
    }
}

impl AsRawFd for PollableStream {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Tcp(stream) => stream.as_raw_fd(),
            Self::Uds(stream) => stream.as_raw_fd(),
        }
    }
}

impl PollableStream {
    pub fn set_nonblocking(&self, nonblocking: bool) -> std::result::Result<(), std::io::Error> {
        match self {
            Self::Tcp(stream) => stream.set_nonblocking(nonblocking),
            Self::Uds(stream) => stream.set_nonblocking(nonblocking),
        }
    }
}