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

//! Test control socket for the dummy devices.
//!
//! The end-to-end tests need to make a device misbehave while it is running, not only at
//! config load, so each dummy device can listen on a loopback TCP port. The protocol is
//! one line in and one line out:
//!
//! ```text
//! <key>            read, reply `key=value`
//! <key>:<value>    write, reply `ok`
//! ```
//!
//! The keys are the ones in [`DummyControl`]. The replies are not needed for correctness,
//! since the tests poll the websocket getters until the engine reacts, but without them a
//! mistyped key fails as a confusing timeout instead of an obvious error.
//!
//! The listener is owned by the device and dies with it, so no state survives a config
//! reload and the tests need no reset step.

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// How often the listener checks whether the device it belongs to has stopped.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long to keep retrying the bind before giving up and running without control.
///
/// A config reload restarts the devices on the same ports, and the previous listener may
/// still be releasing them, so a first bind can lose the race.
const BIND_TIMEOUT: Duration = Duration::from_secs(5);

/// The knobs and counters one dummy device exposes on its control socket.
///
/// The device reads these once per chunk, which is far more often than a test writes one,
/// so `Relaxed` ordering is all that is needed.
#[derive(Default)]
pub struct DummyControl {
    stall: AtomicBool,
    silence: AtomicBool,
    drift_ppm: AtomicI32,
    frames: AtomicU64,
    pauses: AtomicU64,
    resyncs: AtomicU64,
    stop: AtomicBool,
}

impl DummyControl {
    /// Whether the device should stop producing or consuming audio.
    pub fn stalled(&self) -> bool {
        self.stall.load(Ordering::Relaxed)
    }

    /// Whether the device should emit silence instead of the configured signal.
    pub fn silenced(&self) -> bool {
        self.silence.load(Ordering::Relaxed)
    }

    /// How far off nominal the device clock runs, in parts per million.
    pub fn drift_ppm(&self) -> i32 {
        self.drift_ppm.load(Ordering::Relaxed)
    }

    /// Count frames produced or consumed.
    pub fn add_frames(&self, frames: usize) {
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
    }

    /// Count a pause message, sent by the capture and received by the playback.
    pub fn count_pause(&self) {
        self.pauses.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a pacer resync, which is what an overrun or underrun looks like here.
    pub fn count_resync(&self) {
        self.resyncs.fetch_add(1, Ordering::Relaxed);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Owns the control state, and the listener thread when the device config gave a port.
///
/// Dropping this stops the listener, so every exit path out of a device loop releases the
/// port, not just the tidy one.
pub struct ControlListener {
    control: Arc<DummyControl>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ControlListener {
    /// Start listening on `port`, or hand out unconnected control state if there is none.
    pub fn start(port: Option<u16>, name: &'static str) -> Self {
        let control = Arc::new(DummyControl::default());
        let handle = port.map(|port| {
            let control = control.clone();
            thread::Builder::new()
                .name(format!("DummyControl{name}"))
                .spawn(move || listen(port, name, &control))
                .unwrap()
        });
        ControlListener { control, handle }
    }

    /// The state the device loop reads and updates.
    pub fn control(&self) -> Arc<DummyControl> {
        self.control.clone()
    }
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or(());
        }
    }
}

/// Bind the port, retrying while the previous device releases it, then serve connections.
fn listen(port: u16, name: &str, control: &DummyControl) {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + BIND_TIMEOUT;
    let listener = loop {
        // Bind to loopback only. Windows Defender Firewall prompts for sockets that
        // accept external traffic, and a device that can be made to fail on command has
        // no business being reachable from off the machine.
        match TcpListener::bind(address) {
            Ok(listener) => break listener,
            Err(err) => {
                if control.stopped() || Instant::now() >= deadline {
                    warn!("Dummy {name} control socket could not bind to {address}: {err}");
                    return;
                }
                thread::sleep(POLL_INTERVAL);
            }
        }
    };
    if listener.set_nonblocking(true).is_err() {
        warn!("Dummy {name} control socket could not be set to non-blocking");
        return;
    }
    debug!("Dummy {name} control socket listening on {address}");
    // A blocking accept never wakes when the device stops, so poll the stop flag instead.
    while !control.stopped() {
        match listener.accept() {
            Ok((stream, _)) => serve(stream, control),
            Err(err) if err.kind() == ErrorKind::WouldBlock => thread::sleep(POLL_INTERVAL),
            Err(err) => {
                warn!("Dummy {name} control socket stopped accepting: {err}");
                return;
            }
        }
    }
    debug!("Dummy {name} control socket closed");
}

/// Answer commands on one connection until the client closes it or the device stops.
///
/// Connections are served one at a time, which is all the tests need since their client
/// opens a connection, sends a line, reads the reply and closes again.
fn serve(stream: TcpStream, control: &DummyControl) {
    // The accepted socket inherits the listener's non-blocking mode on some platforms and
    // not others, so set both that and the timeout explicitly.
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(POLL_INTERVAL)).is_err()
    {
        return;
    }
    let mut reader = &stream;
    let mut writer = &stream;
    let mut pending = String::new();
    let mut buffer = [0u8; 256];
    while !control.stopped() {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                pending.push_str(&String::from_utf8_lossy(&buffer[..count]));
                while let Some(end) = pending.find('\n') {
                    let line: String = pending.drain(..=end).collect();
                    let reply = handle_line(control, line.trim());
                    if writeln!(writer, "{reply}").is_err() {
                        return;
                    }
                }
            }
            // Windows reports a read timeout as TimedOut, unix as WouldBlock.
            Err(err)
                if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut => {}
            Err(_) => return,
        }
    }
}

/// Handle one line, and return the line to reply with.
fn handle_line(control: &DummyControl, line: &str) -> String {
    match line.split_once(':') {
        Some((key, value)) => write_key(control, key.trim(), value.trim()),
        None => read_key(control, line),
    }
}

fn read_key(control: &DummyControl, key: &str) -> String {
    let value = match key {
        "stall" => u64::from(control.stalled()),
        "silence" => u64::from(control.silenced()),
        "drift" => return format!("drift={}", control.drift_ppm()),
        "frames" => control.frames.load(Ordering::Relaxed),
        "pauses" => control.pauses.load(Ordering::Relaxed),
        "resyncs" => control.resyncs.load(Ordering::Relaxed),
        _ => return format!("unknown key: {key}"),
    };
    format!("{key}={value}")
}

fn write_key(control: &DummyControl, key: &str, value: &str) -> String {
    match key {
        "stall" => store_bool(&control.stall, value),
        "silence" => store_bool(&control.silence, value),
        "drift" => match value.parse::<i32>() {
            Ok(parsed) => {
                control.drift_ppm.store(parsed, Ordering::Relaxed);
                "ok".to_string()
            }
            Err(_) => format!("bad value: {value}"),
        },
        "frames" | "pauses" | "resyncs" => format!("read only: {key}"),
        _ => format!("unknown key: {key}"),
    }
}

fn store_bool(flag: &AtomicBool, value: &str) -> String {
    match value {
        "0" => flag.store(false, Ordering::Relaxed),
        "1" => flag.store(true, Ordering::Relaxed),
        _ => return format!("bad value: {value}"),
    }
    "ok".to_string()
}

#[cfg(test)]
mod tests {
    use super::{DummyControl, handle_line};

    #[test]
    fn writing_a_key_changes_what_reading_it_gives() {
        let control = DummyControl::default();
        assert_eq!(handle_line(&control, "stall"), "stall=0");
        assert_eq!(handle_line(&control, "stall:1"), "ok");
        assert!(control.stalled());
        assert_eq!(handle_line(&control, "stall"), "stall=1");
        assert_eq!(handle_line(&control, "drift: -250"), "ok");
        assert_eq!(handle_line(&control, "drift"), "drift=-250");
    }

    #[test]
    fn counters_are_read_only() {
        let control = DummyControl::default();
        control.add_frames(1024);
        assert_eq!(handle_line(&control, "frames"), "frames=1024");
        assert_eq!(handle_line(&control, "frames:0"), "read only: frames");
        assert_eq!(handle_line(&control, "frames"), "frames=1024");
    }

    #[test]
    fn bad_input_says_what_was_wrong() {
        let control = DummyControl::default();
        assert_eq!(handle_line(&control, "nonsense"), "unknown key: nonsense");
        assert_eq!(handle_line(&control, "nonsense:1"), "unknown key: nonsense");
        assert_eq!(handle_line(&control, "stall:maybe"), "bad value: maybe");
        assert_eq!(handle_line(&control, "drift:lots"), "bad value: lots");
    }
}
