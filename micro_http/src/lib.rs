// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
/////////#![deny(missing_docs)]
//! Minimal implementation of the [HTTP/1.0](https://tools.ietf.org/html/rfc1945)
//! and [HTTP/1.1](https://www.ietf.org/rfc/rfc2616.txt) protocols.
//!
//! HTTP/1.1 has a mandatory header **Host**, but as this crate is only used
//! for parsing API requests, this header (if present) is ignored.
//!
//! This HTTP implementation is stateless thus it does not support chunking or
//! compression.
//!
//! ## Supported Headers
//! The **micro_http** crate has support for parsing the following **Request**
//! headers:
//! - Content-Length
//! - Expect
//! - Transfer-Encoding
//!
//! The **Response** does not have a public interface for adding headers, but whenever
//! a write to the **Body** is made, the headers **ContentLength** and **MediaType**
//! are automatically updated.
//!
//! ### Media Types
//! The supported media types are:
//! - text/plain
//! - application/json
//!
//! ## Supported Methods
//! The supported HTTP Methods are:
//! - GET
//! - PUT
//! - PATCH
//!
//! ## Supported Status Codes
//! The supported status codes are:
//!
//! - Continue - 100
//! - OK - 200
//! - No Content - 204
//! - Bad Request - 400
//! - Not Found - 404
//! - Internal Server Error - 500
//! - Not Implemented - 501
//!
//! ## Example for parsing an HTTP Request from a slice
//! ```
//! extern crate micro_http;
//! use micro_http::{Message, Request, Version};
//!
//! let http_request = Request::try_from(b"GET http://localhost/home HTTP/1.0\r\n\r\n").unwrap();
//! assert_eq!(http_request.version(), Version::Http10);
//! assert_eq!(http_request.uri().get_abs_path(), "/home");
//! ```
//!
//! ## Example for creating an HTTP Response
//! ```
//! extern crate micro_http;
//! use micro_http::{Body, MediaType, Message, Response, StatusCode, Version};
//!
//! let mut response = Response::new(Version::Http10, StatusCode::OK);
//! let body = String::from("This is a test");
//! response.with_body(body.as_bytes())
//!         .with_header("Content-Type".to_string(), "text/plain".to_string());
//!
//! assert!(response.status() == StatusCode::OK);
//! assert_eq!(response.body().unwrap().as_slice(), body.as_bytes());
//! assert_eq!(response.version(), Version::Http10);
//!
//! let mut response_buf: [u8; 126] = [0; 126];
//! assert!(response.send(&mut response_buf.as_mut()).is_ok());
//! ```
//!
//! `HttpConnection` can be used for automatic data exchange and parsing when
//! handling a client, but it only supports one stream.
//!
//! For handling multiple clients use `HttpServer`, which multiplexes `HttpConnection`s
//! and offers an easy to use interface. The server can run in either blocking or
//! non-blocking mode. Non-blocking is achieved by using `epoll` to make sure
//! `requests` will never block when called.
//!
//! ## Example for using the server
//!
//! ```
//! extern crate micro_http;
//! use micro_http::{HttpServer, Message, Response, StatusCode};
//!
//! let path_to_socket = "/tmp/example.sock";
//! std::fs::remove_file(path_to_socket).unwrap_or_default();
//!
//! // Start the server.
//! let mut server = HttpServer::new_uds(path_to_socket).unwrap();
//! server.start_server().unwrap();
//!
//! // Connect a client to the server so it doesn't block in our example.
//! let mut socket = std::os::unix::net::UnixStream::connect(path_to_socket).unwrap();
//!
//! // Server loop processing requests.
//! loop {
//!     for request in server.requests().unwrap() {
//!         let response = request.process(|request| {
//!             // Your code here.
//!             Response::new(request.version(), StatusCode::NoContent)
//!         });
//!         server.respond(response);
//!     }
//!     // Break this example loop.
//!     break;
//! }
//! ```

extern crate libc;

mod client;
mod common;
mod connection;
mod request;
mod response;
mod server;
use common::ascii;
use common::headers;

pub use client::Client;
pub use connection::HttpConnection;
pub use request::{Request, RequestError};
pub use response::{Response, ResponseError, StatusCode};
pub use server::{HttpServer, ServerError};

pub use common::headers::{Headers, MediaType};
pub use common::message::Message;
pub use common::{Body, MessageError, Method, Version};
