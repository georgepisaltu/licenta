use std::net::TcpStream;

use common::message::Message;
use common::{ClientError, MessageError};
use request::Request;
use response::Response;

pub struct Client {
    socket: TcpStream,
    addr: String,
    buffer: Vec<u8>,
}

impl Client {
    pub fn new(url: String, buffer_size: usize) -> Result<Client, ClientError> {
        let stream = TcpStream::connect(url.clone()).map_err(ClientError::StreamError)?;
        Ok(Client {
            socket: stream,
            addr: url,
            buffer: Vec::with_capacity(buffer_size),
        })
    }

    pub fn request(&mut self, mut request: Request) -> Result<Response, MessageError> {
        request
            .send(&mut self.socket)
            .map_err(|_| MessageError::IOError)?;
        Ok(Response::receive(&mut self.socket)?)
    }
}
