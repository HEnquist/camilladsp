// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2025 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

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
