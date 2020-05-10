// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::result::Result;
use std::collections::HashMap;
use std::io::{Error as WriteError, Write};

use RequestError;

/// Wrapper over an HTTP Header type.
#[derive(Debug, Eq, Hash, PartialEq)]
pub enum Header {
    /// Header `Content-Length`.
    ContentLength,
    /// Header `Content-Type`.
    ContentType,
    /// Header `Expect`.
    Expect,
    /// Header `Transfer-Encoding`.
    TransferEncoding,
    /// Header `Server`.
    Server,
}

impl Header {
    /// Returns a byte slice representation of the object.
    pub fn raw(&self) -> &'static [u8] {
        match self {
            Self::ContentLength => b"Content-Length",
            Self::ContentType => b"Content-Type",
            Self::Expect => b"Expect",
            Self::TransferEncoding => b"Transfer-Encoding",
            Self::Server => b"Server",
        }
    }

    /// Parses a byte slice into a Header structure. Header must be ASCII, so also
    /// UTF-8 valid.
    ///
    /// # Errors
    /// `InvalidRequest` is returned if slice contains invalid utf8 characters.
    /// `InvalidHeader` is returned if unsupported header found.
    fn try_from(string: &[u8]) -> Result<Self, RequestError> {
        if let Ok(mut utf8_string) = String::from_utf8(string.to_vec()) {
            utf8_string.make_ascii_lowercase();
            match utf8_string.trim() {
                "content-length" => Ok(Self::ContentLength),
                "content-type" => Ok(Self::ContentType),
                "expect" => Ok(Self::Expect),
                "transfer-encoding" => Ok(Self::TransferEncoding),
                "server" => Ok(Self::Server),
                _ => Err(RequestError::InvalidHeader),
            }
        } else {
            Err(RequestError::InvalidRequest)
        }
    }
}

/// Wrapper over the list of headers associated with a Request that we need
/// in order to parse the request correctly and be able to respond to it.
///
/// The only `Content-Type`s supported are `text/plain` and `application/json`, which are both
/// in plain text actually and don't influence our parsing process.
///
/// All the other possible header fields are not necessary in order to serve this connection
/// and, thus, are not of interest to us. However, we still look for header fields that might
/// invalidate our request as we don't support the full set of HTTP/1.1 specification.
/// Such header entries are "Transfer-Encoding: identity; q=0", which means a compression
/// algorithm is applied to the body of the request, or "Expect: 103-checkpoint".
#[derive(Debug, Default)]
pub struct Headers {
    /// The `Content-Length` header field tells us how many bytes we need to receive
    /// from the source after the headers.
    content_length: i32,
    map: HashMap<String, String>,
}

impl Headers {
    /// Expects one header line and parses it, updating the header structure or returning an
    /// error if the header is invalid.
    ///
    /// # Errors
    /// `UnsupportedHeader` is returned when the parsed header line is not of interest
    /// to us or when it is unrecognizable.
    /// `InvalidHeader` is returned when the parsed header is formatted incorrectly or suggests
    /// that the client is using HTTP features that we do not support in this implementation,
    /// which invalidates the request.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate micro_http;
    /// use micro_http::Headers;
    ///
    /// let mut request_header = Headers::default();
    /// assert!(request_header.parse_header_line(b"Content-Length: 24").is_ok());
    /// assert!(request_header.parse_header_line(b"Content-Length: 24: 2").is_err());
    /// ```
    pub fn parse_header_line(&mut self, header_line: &[u8]) -> Result<(), RequestError> {
        // Headers must be ASCII, so also UTF-8 valid.
        match std::str::from_utf8(header_line) {
            Ok(headers_str) => {
                let entry = headers_str.split(": ").collect::<Vec<&str>>();
                if entry.len() != 2 {
                    return Err(RequestError::InvalidHeader);
                }

                if entry[0].to_lowercase() == "content-length" {
                    match entry[1].trim().parse::<i32>() {
                        Ok(content_length) => {
                            self.content_length = content_length;
                            Ok(())
                        }
                        Err(_) => Err(RequestError::InvalidHeader),
                    }
                } else {
                    self.map.insert(entry[0].to_string(), entry[1].to_string());
                    Ok(())
                }
            }
            _ => Err(RequestError::InvalidHeader),
        }
    }

    /// Returns the content length of the body.
    pub fn content_length(&self) -> i32 {
        self.content_length
    }

    pub fn header_line(&self, key: &str) -> Option<&String> {
        self.map.get(key)
    }

    pub fn add_header_line(&mut self, key: String, value: String) {
        self.map.insert(key, value);
    }

    /// Parses a byte slice into a Headers structure for a HTTP request.
    ///
    /// The byte slice is expected to have the following format: </br>
    ///     * Request Header Lines "<header_line> CRLF"- Optional </br>
    /// There can be any number of request headers, including none, followed by
    /// an extra sequence of Carriage Return and Line Feed.
    /// All header fields are parsed. However, only the ones present in the
    /// [`Headers`](struct.Headers.html) struct are relevant to us and stored
    /// for future use.
    ///
    /// # Errors
    /// The function returns `InvalidHeader` when parsing the byte stream fails.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate micro_http;
    /// use micro_http::Headers;
    ///
    /// let request_headers = Headers::try_from(b"Content-Length: 55\r\n\r\n");
    /// ```
    pub fn try_from(bytes: &[u8]) -> Result<Headers, RequestError> {
        // Headers must be ASCII, so also UTF-8 valid.
        if let Ok(text) = std::str::from_utf8(bytes) {
            let mut headers = Self::default();

            let header_lines = text.split("\r\n");
            for header_line in header_lines {
                if header_line.is_empty() {
                    break;
                }
                match headers.parse_header_line(header_line.as_bytes()) {
                    Ok(_) | Err(RequestError::UnsupportedHeader) => continue,
                    Err(e) => return Err(e),
                };
            }
            return Ok(headers);
        }
        Err(RequestError::InvalidRequest)
    }

    pub fn write_all<T: Write>(&self, mut buf: T) -> Result<(), WriteError> {
        for (key, value) in &self.map {
            buf.write_all(key.as_bytes())?;
            buf.write_all(b": ")?;
            buf.write_all(value.as_bytes())?;
            buf.write_all(b"\r\n")?;
        }
        if self.content_length > 0 {
            buf.write_all(b"Content-Length: ")?;
            buf.write_all(self.content_length.to_string().as_bytes())?;
            buf.write_all(b"\r\n")?;
        }
        buf.write_all(b"\r\n")?;

        Ok(())
    }
}

/// Wrapper over supported Media Types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaType {
    /// Media Type: "text/plain".
    PlainText,
    /// Media Type: "application/json".
    ApplicationJson,
}

impl Default for MediaType {
    /// Default value for MediaType is application/json
    fn default() -> Self {
        Self::ApplicationJson
    }
}

impl MediaType {
    /// Parses a byte slice into a MediaType structure for a HTTP request. MediaType
    /// must be ASCII, so also UTF-8 valid.
    ///
    /// # Errors
    /// The function returns `InvalidRequest` when parsing the byte stream fails or
    /// unsupported MediaType found.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate micro_http;
    /// use micro_http::MediaType;
    ///
    /// assert!(MediaType::try_from(b"application/json").is_ok());
    /// assert!(MediaType::try_from(b"application/json2").is_err());
    /// ```
    pub fn try_from(bytes: &[u8]) -> Result<Self, RequestError> {
        if bytes.is_empty() {
            return Err(RequestError::InvalidRequest);
        }
        let utf8_slice =
            String::from_utf8(bytes.to_vec()).map_err(|_| RequestError::InvalidRequest)?;
        match utf8_slice.as_str().trim() {
            "text/plain" => Ok(Self::PlainText),
            "application/json" => Ok(Self::ApplicationJson),
            _ => Err(RequestError::InvalidRequest),
        }
    }

    /// Returns a static string representation of the object.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate micro_http;
    /// use micro_http::MediaType;
    ///
    /// let media_type = MediaType::ApplicationJson;
    /// assert_eq!(media_type.as_str(), "application/json");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlainText => "text/plain",
            Self::ApplicationJson => "application/json",
        }
    }
}