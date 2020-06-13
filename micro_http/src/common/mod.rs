// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::fmt::{Display, Error, Formatter};

pub mod epoll;
pub mod headers;
pub mod message;

pub mod ascii {
    pub const CR: u8 = b'\r';
    pub const COLON: u8 = b':';
    pub const LF: u8 = b'\n';
    pub const SP: u8 = b' ';
    pub const CRLF_LEN: usize = 2;
}

/// Errors associated with parsing the HTTP Request from a u8 slice.
#[derive(Debug, PartialEq)]
pub enum MessageError {
    /// Request specific error.
    InvalidRequest(RequestError),
    /// Response specific error.
    InvalidResponse(ResponseError),
    /// The HTTP Version in the Request is not supported or it is invalid.
    InvalidHttpVersion(&'static str),
    /// The header specified may be valid, but is not supported by this HTTP implementation.
    UnsupportedHeader,
    /// Header specified is invalid.
    InvalidHeader,
    /// IO error.
    IOError,
}

impl Display for MessageError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::InvalidRequest(inner) => write!(f, "Request error: {}", inner),
            Self::InvalidResponse(inner) => write!(f, "Response error: {}", inner),
            Self::InvalidHttpVersion(inner) => write!(f, "Invalid HTTP Version: {}", inner),
            Self::UnsupportedHeader => write!(f, "Unsupported header."),
            Self::InvalidHeader => write!(f, "Invalid header."),
            Self::IOError => write!(f, "IO error."),
        }
    }
}

/// Errors associated with parsing the HTTP Request from a u8 slice.
#[derive(Debug, PartialEq)]
pub enum RequestError {
    /// The HTTP Method is not supported or it is invalid.
    InvalidHttpMethod(&'static str),
    /// Request URI is invalid.
    InvalidUri(&'static str),
    /// The Request is invalid and cannot be served.
    InvalidRequest,
}

impl Display for RequestError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::InvalidHttpMethod(inner) => write!(f, "Invalid HTTP Method: {}", inner),
            Self::InvalidUri(inner) => write!(f, "Invalid URI: {}", inner),
            Self::InvalidRequest => write!(f, "Invalid request."),
        }
    }
}

/// Errors associated with parsing the HTTP Response from a u8 slice.
#[derive(Debug, PartialEq)]
pub enum ResponseError {
    /// Request Status Code is invalid.
    InvalidStatusCode(&'static str),
    /// The Response is invalid and cannot be parsed.
    InvalidResponse,
}

impl Display for ResponseError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::InvalidStatusCode(inner) => write!(f, "Invalid Status Code: {}", inner),
            Self::InvalidResponse => write!(f, "Invalid response."),
        }
    }
}

/// Errors associated with a HTTP Connection.
#[derive(Debug)]
pub enum ConnectionError {
    /// The request parsing has failed.
    ParseError(MessageError),
    /// Could not perform a stream operation successfully.
    StreamError(std::io::Error),
    /// Attempted to read or write on a closed connection.
    ConnectionClosed,
    /// Attempted to write on a stream when there was nothing to write.
    InvalidWrite,
}

impl Display for ConnectionError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::ParseError(inner) => write!(f, "Parsing error: {}", inner),
            Self::StreamError(inner) => write!(f, "Stream error: {}", inner),
            Self::ConnectionClosed => write!(f, "Connection closed."),
            Self::InvalidWrite => write!(f, "Invalid write attempt."),
        }
    }
}

/// Errors associated with a HTTP Client.
#[derive(Debug)]
pub enum ClientError {
    /// Could not perform a stream operation successfully.
    StreamError(std::io::Error),
}

impl Display for ClientError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::StreamError(inner) => write!(f, "Stream error: {}", inner),
        }
    }
}

/// Errors pertaining to `HttpServer`.
#[derive(Debug)]
pub enum ServerError {
    /// Epoll operations failed.
    IOError(std::io::Error),
    /// Error from one of the connections.
    ConnectionError(ConnectionError),
    /// Server maximum capacity has been reached.
    ServerFull,
}

impl Display for ServerError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::IOError(inner) => write!(f, "IO error: {}", inner),
            Self::ConnectionError(inner) => write!(f, "Connection error: {}", inner),
            Self::ServerFull => write!(f, "Server is full."),
        }
    }
}

/// The Body associated with an HTTP Request or Response.
///
/// ## Examples
/// ```
/// extern crate micro_http;
/// use micro_http::Body;
/// let body = Body::new("This is a test body.".to_string());
/// assert_eq!(body.raw(), b"This is a test body.");
/// assert_eq!(body.len(), 20);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// Body of the HTTP message as bytes.
    pub stream: Vec<u8>,
}

impl Body {
    /// Creates a new `Body` from a `String` input.
    pub fn new<T: Into<Vec<u8>>>(body: T) -> Self {
        Self {
            stream: body.into(),
        }
    }

    pub fn as_stream(&mut self) -> &mut Vec<u8> {
        &mut self.stream
    }

    /// Returns the length of the `Body`.
    pub fn len(&self) -> usize {
        self.stream.len()
    }

    /// Checks if the body is empty, ie with zero length
    pub fn is_empty(&self) -> bool {
        self.stream.len() == 0
    }
}

/// Supported HTTP Methods.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    /// GET Method.
    Get,
    /// PUT Method.
    Put,
    /// PATCH Method.
    Patch,
}

impl Method {
    /// Returns a `Method` object if the parsing of `bytes` is successful.
    ///
    /// The method is case sensitive. A call to try_from with the input b"get" will return
    /// an error, but when using the input b"GET", it returns Method::Get.
    ///
    /// # Errors
    /// `InvalidHttpMethod` is returned if the specified HTTP method is unsupported.
    pub fn try_from(bytes: &[u8]) -> Result<Self, MessageError> {
        match bytes {
            b"GET" => Ok(Self::Get),
            b"PUT" => Ok(Self::Put),
            b"PATCH" => Ok(Self::Patch),
            _ => Err(MessageError::InvalidRequest(
                RequestError::InvalidHttpMethod("Unsupported HTTP method."),
            )),
        }
    }

    /// Returns an `u8 slice` corresponding to the Method.
    pub fn raw(self) -> &'static [u8] {
        match self {
            Self::Get => b"GET",
            Self::Put => b"PUT",
            Self::Patch => b"PATCH",
        }
    }
}

/// Supported HTTP Versions.
///
/// # Examples
/// ```
/// extern crate micro_http;
/// use micro_http::Version;
/// let version = Version::try_from(b"HTTP/1.1");
/// assert!(version.is_ok());
///
/// let version = Version::try_from(b"http/1.1");
/// assert!(version.is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Version {
    /// HTTP/1.0
    Http10,
    /// HTTP/1.1
    Http11,
}

impl Default for Version {
    /// Returns the default HTTP version = HTTP/1.1.
    fn default() -> Self {
        Self::Http11
    }
}

impl Version {
    /// HTTP Version as an `u8 slice`.
    pub fn raw(self) -> &'static [u8] {
        match self {
            Self::Http10 => b"HTTP/1.0",
            Self::Http11 => b"HTTP/1.1",
        }
    }

    /// Creates a new HTTP Version from an `u8 slice`.
    ///
    /// The supported versions are HTTP/1.0 and HTTP/1.1.
    /// The version is case sensitive and the accepted input is upper case.
    ///
    /// # Errors
    /// Returns a `InvalidHttpVersion` when the HTTP version is not supported.
    pub fn try_from(bytes: &[u8]) -> Result<Self, MessageError> {
        match bytes {
            b"HTTP/1.0" => Ok(Self::Http10),
            b"HTTP/1.1" => Ok(Self::Http11),
            _ => Err(MessageError::InvalidHttpVersion(
                "Unsupported HTTP version.",
            )),
        }
    }
}
