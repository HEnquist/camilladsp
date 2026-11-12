// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
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

use crate::file_backend::device::{ReadResult, Reader};

pub struct NonBlockingReader<'a, R: 'a> {
    poll: [nix::poll::PollFd<'a>; 1],
    signals: nix::sys::signal::SigSet,
    timeout: Option<nix::sys::time::TimeSpec>,
    timelimit: time::Duration,
    /// The size of one frame in bytes: one sample for each channel.
    frame_bytes: usize,
    /// The start of a frame that a timed out read stopped inside. A timeout hands over whole
    /// frames only, and these bytes begin the next read, so the stream stays aligned to frames.
    partial: Vec<u8>,
    inner: R,
}

impl<'a, R: Read + AsRawFd + 'a> NonBlockingReader<'a, R> {
    pub fn new(inner: R, timeout_millis: u64, frame_bytes: usize) -> Self {
        let flags = nix::poll::PollFlags::POLLIN;
        let poll: nix::poll::PollFd<'_> =
            nix::poll::PollFd::new(unsafe { BorrowedFd::borrow_raw(inner.as_raw_fd()) }, flags);
        let mut signals = nix::sys::signal::SigSet::empty();
        signals.add(nix::sys::signal::Signal::SIGIO);
        let timelimit = time::Duration::from_millis(timeout_millis);
        let timeout = nix::sys::time::TimeSpec::from_duration(timelimit);
        NonBlockingReader {
            poll: [poll],
            signals,
            timeout: Some(timeout),
            timelimit,
            frame_bytes: frame_bytes.max(1),
            partial: Vec::new(),
            inner,
        }
    }

    /// A read that timed out hands over the whole frames it read. The bytes of a frame the
    /// writer was part-way through are kept for the next read: handing them over would leave
    /// every later frame read from the wrong position in the stream.
    fn timed_out(&mut self, data: &[u8], bytes_read: usize) -> ReadResult {
        let whole = bytes_read - bytes_read % self.frame_bytes;
        self.partial.extend_from_slice(&data[whole..bytes_read]);
        ReadResult::Timeout(whole)
    }
}

impl<'a, R: Read + AsRawFd + 'a> Reader for NonBlockingReader<'a, R> {
    fn read(&mut self, data: &mut [u8]) -> Result<ReadResult, Box<dyn Error>> {
        // the start of a frame left over by a timed out read comes first
        let carried = self.partial.len().min(data.len());
        data[..carried].copy_from_slice(&self.partial[..carried]);
        self.partial.drain(..carried);
        let mut bytes_read = carried;
        let start = time::Instant::now();
        loop {
            match nix::poll::ppoll(&mut self.poll, self.timeout, Some(self.signals)) {
                Ok(0) => return Ok(self.timed_out(data, bytes_read)),
                Ok(_) => {
                    let n = self.inner.read(&mut data[bytes_read..]);
                    match n {
                        Ok(0) => return Ok(ReadResult::EndOfFile(bytes_read)),
                        Ok(n) => {
                            bytes_read += n;
                        }
                        Err(ref e) if e.kind() == ErrorKind::Interrupted => {
                            debug!("got Interrupted");
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => return Err(Box::new(e)),
                    }
                }
                // ppoll is never restarted after a signal handler runs, so a signal
                // such as SIGHUP for a config reload ends up here as EINTR. It goes on
                // to the time limit check below like any other pass.
                Err(nix::errno::Errno::EINTR) => debug!("poll was interrupted"),
                Err(e) => return Err(Box::new(e)),
            }
            if bytes_read == data.len() {
                return Ok(ReadResult::Complete(bytes_read));
            } else if start.elapsed() > self.timelimit {
                return Ok(self.timed_out(data, bytes_read));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    /// Eight bytes in a frame: two channels of 32-bit samples.
    const FRAME: usize = 8;

    /// A stream of `n` bytes, each its own position, so a misaligned read shows.
    fn stream(n: usize) -> Vec<u8> {
        (0..n).map(|i| i as u8).collect()
    }

    fn reader(inner: UnixStream) -> NonBlockingReader<'static, UnixStream> {
        NonBlockingReader::new(inner, 20, FRAME)
    }

    #[test]
    fn whole_frames_are_read_as_before() {
        let (mut writer, inner) = UnixStream::pair().unwrap();
        let mut r = reader(inner);
        let sent = stream(4 * FRAME);
        writer.write_all(&sent).unwrap();
        let mut data = vec![0u8; 4 * FRAME];
        assert!(matches!(
            r.read(&mut data).unwrap(),
            ReadResult::Complete(32)
        ));
        assert_eq!(data, sent);
    }

    #[test]
    fn a_read_that_times_out_inside_a_frame_hands_over_whole_frames_only() {
        let (mut writer, inner) = UnixStream::pair().unwrap();
        let mut r = reader(inner);
        let sent = stream(6 * FRAME);
        // two and a half frames, then the writer pauses past the timeout
        writer.write_all(&sent[..20]).unwrap();
        let mut data = vec![0u8; 4 * FRAME];
        assert!(matches!(
            r.read(&mut data).unwrap(),
            ReadResult::Timeout(16)
        ));
        assert_eq!(data[..16], sent[..16]);
        // the rest of the third frame and two more: the next read starts where the stream is
        writer.write_all(&sent[20..]).unwrap();
        let mut data = vec![0u8; 4 * FRAME];
        assert!(matches!(
            r.read(&mut data).unwrap(),
            ReadResult::Complete(32)
        ));
        assert_eq!(data, sent[16..48]);
    }

    #[test]
    fn a_part_frame_alone_is_a_timeout_with_nothing_and_begins_the_next_read() {
        let (mut writer, inner) = UnixStream::pair().unwrap();
        let mut r = reader(inner);
        let sent = stream(2 * FRAME);
        writer.write_all(&sent[..3]).unwrap();
        let mut data = vec![0u8; 2 * FRAME];
        assert!(matches!(r.read(&mut data).unwrap(), ReadResult::Timeout(0)));
        writer.write_all(&sent[3..]).unwrap();
        assert!(matches!(
            r.read(&mut data).unwrap(),
            ReadResult::Complete(16)
        ));
        assert_eq!(data, sent);
    }

    #[test]
    fn the_end_of_the_stream_hands_over_what_there_is() {
        let (mut writer, inner) = UnixStream::pair().unwrap();
        let mut r = reader(inner);
        let sent = stream(20);
        writer.write_all(&sent).unwrap();
        drop(writer);
        let mut data = vec![0u8; 4 * FRAME];
        assert!(matches!(
            r.read(&mut data).unwrap(),
            ReadResult::EndOfFile(20)
        ));
        assert_eq!(data[..20], sent[..]);
    }
}
