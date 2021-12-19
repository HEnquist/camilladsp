use std::error::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::time::Duration;

use crate::filedevice::{ReadResult, Reader};

pub struct BlockingReader<R> {
    inner: R,
}

impl<R: Read> BlockingReader<R> {
    pub fn new(inner: R) -> Self {
        BlockingReader { inner }
    }
}

impl<R: Read> Reader for BlockingReader<R> {
    fn read(&mut self, data: &mut [u8]) -> Result<ReadResult, Box<dyn Error>> {
        let requested = data.len();
        let mut buf = &mut *data;
        while !buf.is_empty() {
            match self.inner.read(buf) {
                Ok(0) => break,
                Ok(n) => {
                    let tmp = buf;
                    buf = &mut tmp[n..];
                }
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {
                    debug!("got Interrupted");
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => return Err(Box::new(e)),
            }
        }
        if !buf.is_empty() {
            Ok(ReadResult::EndOfFile(requested - buf.len()))
        } else {
            Ok(ReadResult::Complete(requested))
        }
    }
}
