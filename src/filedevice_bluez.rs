use std::io;
use std::io::Read;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedFd;
use zbus::Message;

use crate::filereader_nonblock::NonBlockingReader;

pub struct WrappedBluezFd {
    pipe_fd: zbus::zvariant::OwnedFd,
    _ctrl_fd: zbus::zvariant::OwnedFd,
    _msg: Arc<Message>,
}

impl WrappedBluezFd {
    fn new_from_open_message(r: Arc<Message>) -> WrappedBluezFd {
        let (pipe_fd, ctrl_fd): (OwnedFd, OwnedFd) = r.body().unwrap();
        return WrappedBluezFd {
            pipe_fd: pipe_fd,
            _ctrl_fd: ctrl_fd,
            _msg: r,
        };
    }
}

impl Read for WrappedBluezFd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        nix::unistd::read(self.pipe_fd.as_raw_fd(), buf).map_err(|e| io::Error::from(e))
    }
}

impl AsRawFd for WrappedBluezFd {
    fn as_raw_fd(&self) -> RawFd {
        return self.pipe_fd.as_raw_fd();
    }
}

pub fn open_bluez_dbus_fd(
    service: String,
    path: String,
    chunksize: usize,
    samplerate: usize,
) -> Result<Box<NonBlockingReader<WrappedBluezFd>>, zbus::Error> {
    let conn1 = Connection::system()?;
    let res = conn1.call_method(Some(service), path, Some("org.bluealsa.PCM1"), "Open", &())?;

    let reader = Box::new(NonBlockingReader::new(
        WrappedBluezFd::new_from_open_message(res),
        2 * 1000 * chunksize as u64 / samplerate as u64,
    ));
    return Ok(reader);
}
