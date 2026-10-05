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

//! A simulated source for the dummy capture, the stand-in for a player on a loopback.
//!
//! The controller follows what feeds a capture, and asks it at startup and while nothing
//! runs. A socket owned by the device can't answer then, since the device only exists
//! during a session. So the source belongs to the test instead: the test runs a small TCP
//! server on `source_port`, and the dummy asks it what the source is doing. One line in,
//! one line out:
//!
//! ```text
//! state    reply `inactive`, `unknown`, or `format <rate> <channels> [<format>]`
//! ```
//!
//! A server that isn't there, or doesn't answer, reads as unknown.
//!
//! While following, the dummy capture behaves like a loopback capture with `PCM Notify`
//! set. At open, a source at another rate or channel count stops the session with a
//! format change instead of opening, like the ALSA open-time check. While running, a
//! source that switches to another rate or channel count stops it the same way, like the
//! kernel stopping the capture, and an inactive source gives silence. The dummy has no
//! sample format, so the format only shows in what the query reports.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

use crate::controller::{SourceFormat, SourceState};

/// How long to wait for the test's server, to connect and then to answer.
const TIMEOUT: Duration = Duration::from_millis(500);

/// How often a running capture asks the source.
const WATCH_INTERVAL: Duration = Duration::from_millis(50);

/// Ask the simulated source on `port` what it is doing.
pub fn query(port: u16) -> SourceState {
    match ask(port) {
        Ok(line) => parse_state(&line),
        Err(err) => {
            debug!("Simulated source on port {port} did not answer: {err}");
            SourceState::Unknown
        }
    }
}

fn ask(port: u16) -> std::io::Result<String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.write_all(b"state\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line)
}

fn parse_state(line: &str) -> SourceState {
    let mut words = line.split_whitespace();
    match words.next() {
        Some("inactive") => SourceState::Inactive,
        Some("format") => {
            let rate = words.next().and_then(|w| w.parse().ok());
            let channels = words.next().and_then(|w| w.parse().ok());
            match (rate, channels) {
                (Some(samplerate), Some(channels)) => SourceState::Format(SourceFormat {
                    samplerate,
                    channels: Some(channels),
                    format: words.next().map(str::to_string),
                }),
                _ => {
                    warn!("Simulated source sent a bad format: {}", line.trim());
                    SourceState::Unknown
                }
            }
        }
        _ => SourceState::Unknown,
    }
}

/// Keeps asking the source from a thread of its own while a capture runs, so the capture
/// loop only reads the latest answer and is never held up by the socket.
///
/// Dropping it stops the thread.
pub struct SourceWatcher {
    state: Arc<Mutex<SourceState>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SourceWatcher {
    /// Start watching, with `first` as the state until the first answer.
    pub fn start(port: u16, first: SourceState) -> Self {
        let state = Arc::new(Mutex::new(first));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let state = state.clone();
            let stop = stop.clone();
            thread::Builder::new()
                .name("DummySource".to_string())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        *state.lock() = query(port);
                        thread::sleep(WATCH_INTERVAL);
                    }
                })
                .unwrap()
        };
        SourceWatcher {
            state,
            stop,
            handle: Some(handle),
        }
    }

    /// What the source was doing at the last answer.
    pub fn state(&self) -> SourceState {
        self.state.lock().clone()
    }
}

impl Drop for SourceWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or(());
        }
    }
}

/// The source format, if the source runs at another rate or channel count than the
/// capture. That is what makes a loopback capture stop, or refuse to open.
pub fn differs(state: &SourceState, samplerate: usize, channels: usize) -> Option<SourceFormat> {
    match state {
        SourceState::Format(format)
            if format.samplerate != samplerate || format.channels != Some(channels) =>
        {
            Some(format.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_parse() {
        assert_eq!(parse_state("inactive\n"), SourceState::Inactive);
        assert_eq!(parse_state("unknown\n"), SourceState::Unknown);
        assert_eq!(parse_state(""), SourceState::Unknown);
        assert_eq!(
            parse_state("format 44100 2 S16_LE\n"),
            SourceState::Format(SourceFormat {
                samplerate: 44100,
                channels: Some(2),
                format: Some("S16_LE".to_string()),
            })
        );
        assert_eq!(
            parse_state("format 96000 4"),
            SourceState::Format(SourceFormat {
                samplerate: 96000,
                channels: Some(4),
                format: None,
            })
        );
        assert_eq!(parse_state("format fast"), SourceState::Unknown);
    }

    #[test]
    fn a_missing_server_is_unknown() {
        // Bind and drop a listener to get a port nothing listens on.
        let port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert_eq!(query(port), SourceState::Unknown);
    }

    #[test]
    fn only_another_rate_or_channel_count_differs() {
        let at = |samplerate, channels| {
            SourceState::Format(SourceFormat {
                samplerate,
                channels: Some(channels),
                format: Some("S32_LE".to_string()),
            })
        };
        assert!(differs(&at(48000, 2), 48000, 2).is_none());
        assert_eq!(differs(&at(44100, 2), 48000, 2).unwrap().samplerate, 44100);
        assert_eq!(differs(&at(48000, 4), 48000, 2).unwrap().channels, Some(4));
        assert!(differs(&SourceState::Inactive, 48000, 2).is_none());
        assert!(differs(&SourceState::Unknown, 48000, 2).is_none());
    }
}
