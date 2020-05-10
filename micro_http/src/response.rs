// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::io::{Error as WriteError, Write};

use ascii::{COLON, CR, LF, SP};
use common::{Body, Version};
use headers::{Header, MediaType, Headers};
use common::message::Message;


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
}

struct StatusLine {
    http_version: Version,
    status_code: StatusCode,
}

impl StatusLine {
    fn new(http_version: Version, status_code: StatusCode) -> Self {
        Self {
            http_version,
            status_code,
        }
    }

    fn write_all<T: Write>(&self, mut buf: T) -> Result<(), WriteError> {
        buf.write_all(self.http_version.raw())?;
        buf.write_all(&[SP])?;
        buf.write_all(self.status_code.raw())?;
        buf.write_all(&[SP, CR, LF])?;

        Ok(())
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
        self.status_line.write_all(out)?;
        // self.headers.write_all(&mut buf)?;
        // self.write_body(&mut buf)?;
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

    fn write_body<T: Write>(&mut self, mut buf: T) -> Result<(), WriteError> {
        if let Some(ref mut body) = self.body {
            buf.write_all(body.as_stream())?;
        }
        Ok(())
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
}
