// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::io::{Error as WriteError, Read, Write};

use ascii::{CR, CRLF_LEN, LF, SP};
use common::message::Message;
pub use common::ResponseError;
use common::{Body, MessageError, Version};
use headers::Headers;
use request::find;

/// Wrapper over a response status code.
///
/// The status code is defined as specified in the
/// [RFC](https://tools.ietf.org/html/rfc7231#section-6).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatusCode {
    /// 100, Continue
    Continue,
    /// 200, OK
    OK,
    /// 204, No Content
    NoContent,
    /// 400, Bad Request
    BadRequest,
    /// 404, Not Found
    NotFound,
    /// 500, Internal Server Error
    InternalServerError,
    /// 501, Not Implemented
    NotImplemented,
}

impl StatusCode {
    /// Returns the status code as bytes.
    pub fn raw(self) -> &'static [u8; 3] {
        match self {
            Self::Continue => b"100",
            Self::OK => b"200",
            Self::NoContent => b"204",
            Self::BadRequest => b"400",
            Self::NotFound => b"404",
            Self::InternalServerError => b"500",
            Self::NotImplemented => b"501",
        }
    }

    pub fn try_from(bytes: &[u8]) -> Result<Self, MessageError> {
        match bytes {
            b"100" => Ok(Self::Continue),
            b"200" => Ok(Self::OK),
            b"204" => Ok(Self::NoContent),
            b"400" => Ok(Self::BadRequest),
            b"404" => Ok(Self::NotFound),
            b"500" => Ok(Self::InternalServerError),
            b"501" => Ok(Self::NotImplemented),
            _ => Err(MessageError::InvalidResponse(
                ResponseError::InvalidStatusCode("Unsupported HTTP status code."),
            )),
        }
    }
}

struct StatusLine {
    http_version: Version,
    status_code: StatusCode,
    status_message: Option<String>,
}

impl StatusLine {
    fn new(http_version: Version, status_code: StatusCode) -> Self {
        Self {
            http_version,
            status_code,
            status_message: None,
        }
    }

    fn write_all<T: Write>(&self, buf: &mut T) -> Result<(), WriteError> {
        buf.write_all(self.http_version.raw())?;
        buf.write_all(&[SP])?;
        buf.write_all(self.status_code.raw())?;
        if let Some(status_text) = &self.status_message {
            buf.write_all(&[SP])?;
            buf.write_all(status_text.as_bytes())?;
        }
        buf.write_all(&[CR, LF])?;

        Ok(())
    }

    fn parse_status_line(status_line: &[u8]) -> (&[u8], &[u8], &[u8]) {
        if let Some(version_end) = find(status_line, &[SP]) {
            let version = &status_line[..version_end];

            let code_and_message = &status_line[(version_end + 1)..];

            if let Some(code_end) = find(code_and_message, &[SP]) {
                let code = &code_and_message[..code_end];

                let message = &code_and_message[(code_end + 1)..];

                return (version, code, message);
            }

            return (version, code_and_message, b"");
        }

        (b"", b"", b"")
    }

    pub fn try_from(status_line: &[u8]) -> Result<Self, MessageError> {
        let (version, code, message) = Self::parse_status_line(status_line);
        let message = if message == b"" {
            None
        } else {
            Some(
                String::from_utf8(message.to_vec())
                    .map_err(|_| MessageError::InvalidResponse(ResponseError::InvalidResponse))?,
            )
        };

        Ok(Self {
            http_version: Version::try_from(version)
                .map_err(|_| MessageError::InvalidResponse(ResponseError::InvalidResponse))?, //todo fix with invalid_http_version
            status_code: StatusCode::try_from(code)?,
            status_message: message,
        })
    }
}

/// Wrapper over an HTTP Response.
///
/// The Response is created using a `Version` and a `StatusCode`. When creating a Response object,
/// the body is initialized to `None` and the header is initialized with the `default` value. The body
/// can be updated with a call to `set_body`. The header can be updated with `set_content_type` and
/// `set_server`.
pub struct Response {
    status_line: StatusLine,
    headers: Headers,
    body: Option<Body>,
}

impl Message for Response {
    fn send<U: Write>(&mut self, out: &mut U) -> Result<(), WriteError> {
        let mut content_length: i32 = 0;
        if let Some(body) = self.body() {
            content_length = body.len() as i32;
        }
        self.headers.set_content_length(content_length);

        self.status_line.write_all(out)?;
        self.headers.write_all(out)?;
        match self.body.as_mut() {
            Some(body) => {
                let mut slice: &[u8] = body.as_stream().as_mut_slice();
                std::io::copy(&mut slice, out)?;
            }
            None => {}
        }
        Ok(())
    }

    fn header_line(&self, key: &String) -> Option<&String> {
        self.headers.header_line(key)
    }

    fn with_header(&mut self, key: String, value: String) -> &mut Self {
        self.headers.add_header_line(key, value);
        self
    }

    fn version(&self) -> Version {
        self.status_line.http_version
    }

    fn body(&mut self) -> Option<&Vec<u8>> {
        if let Some(ref mut body) = self.body {
            Some(body.as_stream())
        } else {
            None
        }
    }

    fn with_body(&mut self, bytes: &[u8]) -> &mut Self {
        self.headers.set_content_length(bytes.len() as i32);
        self.body = Some(Body::new(bytes));
        self
    }
}

impl Response {
    /// Creates a new HTTP `Response` with an empty body.
    pub fn new(http_version: Version, status_code: StatusCode) -> Self {
        Self {
            status_line: StatusLine::new(http_version, status_code),
            headers: Headers::default(),
            body: Default::default(),
        }
    }

    /// Returns the Status Code of the Response.
    pub fn status(&self) -> StatusCode {
        self.status_line.status_code
    }

    /// Returns the HTTP Version of the response.
    pub fn content_length(&self) -> i32 {
        self.headers.content_length()
    }

    /// Returns the HTTP Version of the response.
    pub fn http_version(&self) -> Version {
        self.status_line.http_version
    }

    pub fn receive<U: Read>(input: &mut U) -> Result<Self, MessageError> {
        let mut buf: [u8; 1024] = [0; 1024];
        match input
            .read(&mut buf[..])
            .map_err(|_| MessageError::IOError)?
        {
            0 => {
                return Err(MessageError::InvalidResponse(
                    ResponseError::InvalidResponse,
                ));
            }
            n => {
                if let Some(status_end) = find(&buf[..], &[CR, LF]) {
                    let headers_and_body = &buf[(status_end + CRLF_LEN)..n];
                    if let Some(headers_end) =
                        find(&headers_and_body[..], &[CR, LF, CR, LF])
                    {
                        let mut response = Response {
                            status_line: StatusLine::try_from(&buf[..status_end])?,
                            headers: Headers::try_from(&headers_and_body[..headers_end])?,
                            body: Default::default(),
                        };

                        if response.headers.content_length() != 0 {
                            let body_bytes = &headers_and_body[(headers_end + 2 * CRLF_LEN)..];
                            let mut bytes_left = response.headers.content_length();
                            let mut body: Vec<u8> = Vec::with_capacity(bytes_left as usize);
                            let buffered_body_len = body_bytes.len() as i32;
                            bytes_left -= buffered_body_len;
                            body.write_all(&body_bytes[..])
                                .map_err(|_| MessageError::IOError)?;

                            if bytes_left > 0 {
                                input
                                    .read_exact(&mut body[buffered_body_len as usize..])
                                    .map_err(|_| MessageError::IOError)?;
                            }

                            response.with_body(&body[..]);
                            return Ok(response);
                        }

                        return Ok(response);
                    }
                }
            }
        };

        Err(MessageError::InvalidResponse(
            ResponseError::InvalidResponse,
        ))
    }
}
