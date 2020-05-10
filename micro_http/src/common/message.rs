use std::io::{Error as WriteError, Write};
use common::{Body, Version};

pub trait Message {
    fn send<U: Write>(&mut self, out: &mut U) -> Result<(), WriteError>;
    fn header_line(&self, key: &String) -> Option<&String>;
    fn with_header(&mut self, key: String, value: String) -> &mut Self;
    fn version(&self) -> Version;
    fn body(&mut self) -> Option<&Vec<u8>>;
    fn with_body(&mut self, bytes: &[u8]) -> &mut Self;
}