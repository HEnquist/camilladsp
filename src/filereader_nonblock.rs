use nix;

use std::error::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::time;
use std::time::Duration;

use crate::filedevice::{ReadResult, Reader};

pub struct NonBlockingReader<R> {
    poll: nix::poll::PollFd,
    signals: nix::sys::signal::SigSet,
    timeout: Option<nix::sys::time::TimeSpec>,
    timelimit: time::Duration,
    inner: R,
}

impl<R: Read + AsRawFd> NonBlockingReader<R> {
    pub fn new(inner: R, timeout_millis: u64) -> Self {
        let flags = nix::poll::PollFlags::POLLIN;
        let poll = nix::poll::PollFd::new(inner.as_raw_fd(), flags);
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

impl<R: Read + AsRawFd> Reader for NonBlockingReader<R> {
    fn read(&mut self, data: &mut [u8]) -> Result<ReadResult, Box<dyn Error>> {
        let mut buf = &mut *data;
        let mut bytes_read = 0;
        let start = time::Instant::now();
        loop {
            let res = nix::poll::ppoll(&mut [self.poll], self.timeout, self.signals)?;
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
