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

use nix;

use std::error::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::os::unix::io::{AsRawFd, BorrowedFd};
use std::time;
use std::time::Duration;

use crate::filedevice::{ReadResult, Reader};

pub struct NonBlockingReader<'a, R: 'a> {
    poll: nix::poll::PollFd<'a>,
    signals: nix::sys::signal::SigSet,
    timeout: Option<nix::sys::time::TimeSpec>,
    timelimit: time::Duration,
    inner: R,
}

impl<'a, R: Read + AsRawFd + 'a> NonBlockingReader<'a, R> {
    pub fn new(inner: R, timeout_millis: u64) -> Self {
        let flags = nix::poll::PollFlags::POLLIN;
        let poll: nix::poll::PollFd<'_> =
            nix::poll::PollFd::new(unsafe { BorrowedFd::borrow_raw(inner.as_raw_fd()) }, flags);
        let mut signals = nix::sys::signal::SigSet::empty();
        signals.add(nix::sys::signal::Signal::SIGIO);
        let timelimit = time::Duration::from_millis(timeout_millis);
        let timeout = nix::sys::time::TimeSpec::from_duration(timelimit);
        NonBlockingReader {
            poll,
            signals,
            timeout: Some(timeout),
            timelimit,
            inner,
        }
    }
}

impl<'a, R: Read + AsRawFd + 'a> Reader for NonBlockingReader<'a, R> {
    fn read(&mut self, data: &mut [u8]) -> Result<ReadResult, Box<dyn Error>> {
        let mut buf = &mut *data;
        let mut bytes_read = 0;
        let start = time::Instant::now();
        loop {
            let res = nix::poll::ppoll(&mut [self.poll], self.timeout, Some(self.signals))?;
            //println!("loop...");
            if res == 0 {
                return Ok(ReadResult::Timeout(bytes_read));
            } else {
                let n = self.inner.read(buf);
                match n {
                    Ok(0) => return Ok(ReadResult::EndOfFile(bytes_read)),
                    Ok(n) => {
                        let tmp = buf;
                        buf = &mut tmp[n..];
                        bytes_read += n;
                    }
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => {
                        debug!("got Interrupted");
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => return Err(Box::new(e)),
                }
            }
            if buf.is_empty() {
                return Ok(ReadResult::Complete(bytes_read));
            } else if start.elapsed() > self.timelimit {
                return Ok(ReadResult::Timeout(bytes_read));
            }
        }
    }
}
