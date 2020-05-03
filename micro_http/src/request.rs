// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::str::from_utf8;
use std::io::Write;

use common::ascii::{CR, CRLF_LEN, LF, SP};
pub use common::RequestError;
use common::{Body, Method, Version};
use headers::Headers;
use common::message::Message;

/// Finds the first occurence of `sequence` in the `bytes` slice.
///
/// Returns the starting position of the `sequence` in `bytes` or `None` if the
/// `sequence` is not found.
pub fn find(bytes: &[u8], sequence: &[u8]) -> Option<usize> {
    bytes
        .windows(sequence.len())
        .position(|window| window == sequence)
}

/// Wrapper over HTTP URIs.
///
/// The `Uri` can not be used directly and it is only accessible from an HTTP Request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Uri {
    pub path: String,
}

impl Uri {
    fn new(slice: &str) -> Self {
        Self {
            path: String::from(slice),
        }
    }

    fn try_from(bytes: &[u8]) -> Result<Self, RequestError> {
        if bytes.is_empty() {
            return Err(RequestError::InvalidUri("Empty URI not allowed."));
        }
        let utf8_slice =
            from_utf8(bytes).map_err(|_| RequestError::InvalidUri("Cannot parse URI as UTF-8."))?;
        Ok(Self::new(utf8_slice))
    }

    /// Returns the absolute path of the `Uri`.
    ///
    /// URIs can be represented in absolute form or relative form. The absolute form includes
    /// the HTTP scheme, followed by the absolute path as follows:
    /// "http:" "//" host [ ":" port ] [ abs_path ]
    /// The relative URIs can be one of net_path | abs_path | rel_path.
    /// This method only handles absolute URIs and relative URIs specified by abs_path.
    /// The abs_path is expected to start with '/'.
    ///
    /// # Errors
    /// Returns an empty byte array when the host or the path are empty/invalid.
    pub fn get_abs_path(&self) -> &str {
        const HTTP_SCHEME_PREFIX: &str = "http://";

        if self.path.starts_with(HTTP_SCHEME_PREFIX) {
            let without_scheme = &self.path[HTTP_SCHEME_PREFIX.len()..];
            if without_scheme.is_empty() {
                return "";
            }
            // The host in this case includes the port and contains the bytes after http:// up to
            // the next '/'.
            match without_scheme.bytes().position(|byte| byte == b'/') {
                Some(len) => &without_scheme[len..],
                None => "",
            }
        } else {
            if self.path.starts_with('/') {
                return self.path.as_str();
            }

            ""
        }
    }
}

/// Wrapper over an HTTP Request Line.
#[derive(Debug, PartialEq)]
pub struct RequestLine {
    method: Method,
    uri: Uri,
    http_version: Version,
}

impl RequestLine {
    fn parse_request_line(request_line: &[u8]) -> (&[u8], &[u8], &[u8]) {
        if let Some(method_end) = find(request_line, &[SP]) {
            let method = &request_line[..method_end];

            let uri_and_version = &request_line[(method_end + 1)..];

            if let Some(uri_end) = find(uri_and_version, &[SP]) {
                let uri = &uri_and_version[..uri_end];

                let version = &uri_and_version[(uri_end + 1)..];

                return (method, uri, version);
            }

            return (method, uri_and_version, b"");
        }

        (b"", b"", b"")
    }

    /// Tries to parse a byte stream in a request line. Fails if the request line is malformed.
    ///
    /// # Errors
    /// `InvalidHttpMethod` is returned if the specified HTTP method is unsupported.
    /// `InvalidHttpVersion` is returned if the specified HTTP version is unsupported.
    /// `InvalidUri` is returned if the specified Uri is not valid.
    pub fn try_from(request_line: &[u8]) -> Result<Self, RequestError> {
        let (method, uri, version) = Self::parse_request_line(request_line);

        Ok(Self {
            method: Method::try_from(method)?,
            uri: Uri::try_from(uri)?,
            http_version: Version::try_from(version)?,
        })
    }

    // Returns the minimum length of a valid request. The request must contain
    // the method (GET), the URI (minmum 1 character), the HTTP version(HTTP/DIGIT.DIGIT) and
    // 2 separators (SP).
    fn min_len() -> usize {
        Method::Get.raw().len() + 1 + Version::Http10.raw().len() + 2
    }
}

/// Wrapper over an HTTP Request.
#[allow(unused)]
#[derive(Debug)]
pub struct Request {
    /// The request line of the request.
    pub request_line: RequestLine,
    /// The headers of the request.
    pub headers: Headers,
    /// The body of the request.
    pub body: Option<Body>,
}

impl Message for Request {
    fn send<U: Write>(&mut self, out: &mut U) -> i32 {
        //std::io::copy(&mut self.body.as_readonly_stream(), out);
        0
    }

    fn header_line(&self, key: &String) -> Option<&String> {
        self.headers.header_line(key)
    }

    fn with_header(&mut self, key: String, value: String) -> &mut Self {
        self.headers.with_header_line(key, value);
        self
    }

    fn version(&self) -> Version {
        self.request_line.http_version
    }

    fn body(&mut self) -> Option<&Vec<u8>> {
        if let Some(ref mut body) = self.body {
            Some(body.as_stream())
        } else {
            None
        }
    }

    fn with_body(&mut self, bytes: &[u8]) -> &mut Self {
        self.body = Some(Body::new(bytes));
        self
    }
}

impl Request {
    /// Parses a byte slice into a HTTP Request.
    ///
    /// The byte slice is expected to have the following format: </br>
    ///     * Request Line: "GET SP Request-uri SP HTTP/1.0 CRLF" - Mandatory </br>
    ///     * Request Headers "<headers> CRLF"- Optional </br>
    ///     * Entity Body - Optional </br>
    /// The request headers and the entity body is not parsed and None is returned because
    /// these are not used by the MMDS server.
    /// The only supported method is GET and the HTTP protocol is expected to be HTTP/1.0
    /// or HTTP/1.1.
    ///
    /// # Errors
    /// The function returns InvalidRequest when parsing the byte stream fails.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate micro_http;
    /// use micro_http::Request;
    ///
    /// let http_request = Request::try_from(b"GET http://localhost/home HTTP/1.0\r\n");
    /// ```
    pub fn try_from(byte_stream: &[u8]) -> Result<Self, RequestError> {
        // The first line of the request is the Request Line. The line ending is CR LF.
        let request_line_end = match find(byte_stream, &[CR, LF]) {
            Some(len) => len,
            // If no CR LF is found in the stream, the request format is invalid.
            None => return Err(RequestError::InvalidRequest),
        };

        let request_line_bytes = &byte_stream[..request_line_end];
        if request_line_bytes.len() < RequestLine::min_len() {
            return Err(RequestError::InvalidRequest);
        }

        let request_line = RequestLine::try_from(request_line_bytes)?;

        // Find the next CR LF CR LF sequence in our buffer starting at the end on the Request
        // Line, including the trailing CR LF previously found.
        match find(&byte_stream[request_line_end..], &[CR, LF, CR, LF]) {
            // If we have found a CR LF CR LF at the end of the Request Line, the request
            // is complete.
            Some(0) => Ok(Self {
                request_line,
                headers: Headers::default(),
                body: None,
            }),
            Some(headers_end) => {
                // Parse the request headers.
                // Start by removing the leading CR LF from them.
                let headers_and_body = &byte_stream[(request_line_end + CRLF_LEN)..];
                let headers_end = headers_end - CRLF_LEN;
                let headers = Headers::try_from(&headers_and_body[..headers_end])?;

                // Parse the body of the request.
                // Firstly check if we have a body.
                let body = match headers.content_length() {
                    0 => {
                        // No request body.
                        None
                    }
                    content_length => {
                        // Headers suggest we have a body, but the buffer is shorter than the specified
                        // content length.
                        if headers_and_body.len() - (headers_end + 2 * CRLF_LEN)
                            < content_length as usize
                        {
                            return Err(RequestError::InvalidRequest);
                        }
                        let body_as_bytes = &headers_and_body[(headers_end + 2 * CRLF_LEN)..];
                        // If the actual length of the body is different than the `Content-Length` value
                        // in the headers then this request is invalid.
                        if body_as_bytes.len() == content_length as usize {
                            Some(Body::new(body_as_bytes))
                        } else {
                            return Err(RequestError::InvalidRequest);
                        }
                    }
                };

                Ok(Self {
                    request_line,
                    headers,
                    body,
                })
            }
            // If we can't find a CR LF CR LF even though the request should have headers
            // the request format is invalid.
            None => Err(RequestError::InvalidRequest),
        }
    }

    /// Returns the `Uri` from the parsed `Request`.
    ///
    /// The return value can be used to get the absolute path of the URI.
    pub fn uri(&self) -> &Uri {
        &self.request_line.uri
    }

    /// Returns the HTTP `Method` of the `Request`.
    pub fn method(&self) -> Method {
        self.request_line.method
    }
}