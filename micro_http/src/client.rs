use common::message::Message;
use common::{ClientError, MessageError};
use request::Request;
use response::Response;

use std::io::{Read, Write};

pub struct Client<T> {
    socket: T,
    base_url: String,
}

impl<T: Read + Write> Client<T> {
    pub fn new(stream: T, base_url: String) -> Result<Client<T>, ClientError> {
        Ok(Client {
            socket: stream,
            base_url,
        })
    }

    pub fn request(&mut self, mut request: Request) -> Result<Response, MessageError> {
        request
            .send(&mut self.socket)
            .map_err(|_| MessageError::IOError)?;
        Ok(Response::receive(&mut self.socket)?)
    }

    pub fn base_url(&self) -> &String {
        &self.base_url
    }
}
