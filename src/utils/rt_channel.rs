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

//! A bounded channel for the audio threads that blocks without yielding.
//!
//! A blocking `recv()` or `send()` on a crossbeam channel spins and then calls
//! `thread::yield_now()` a few times before it parks the thread.
//! On macOS that is `sched_yield()`, which can push a real-time thread down for
//! a whole scheduler quantum, around 10 ms.
//! The bounded `std::sync::mpsc` channel parks without yielding since Rust 1.69,
//! but it has no stable `len()` or `send_timeout()`.
//!
//! The non-blocking `try_send()` and `try_recv()` of crossbeam can still yield
//! when several threads send or receive on the same channel at once.
//! Each channel here has one sender thread and one receiver thread, so that doesn't happen.
//!
//! This channel only uses the non-blocking `try_send()` and `try_recv()` of a crossbeam
//! channel, and blocks on a `std` [`Condvar`] instead, which parks the thread directly.
//! The side that does not wait only takes the lock when the other side is waiting,
//! so `try_send()` and `try_recv()` stay lock-free and can be used from an audio callback.
//!
//! The methods and error types match those of crossbeam's channels.

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub use crossbeam_channel::{
    RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError,
};

/// Wakes the threads that wait for one side of the channel.
#[derive(Default)]
struct Signal {
    lock: Mutex<()>,
    cvar: Condvar,
    /// The number of threads that are waiting, or about to wait. Only changed with the lock held.
    waiters: AtomicUsize,
}

impl Signal {
    fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wake the waiting threads after the channel has changed.
    /// Does not touch the lock when nobody waits.
    fn notify(&self) {
        // Pairs with the fence in `wait`. Either this sees the waiter,
        // or the waiter sees the change to the channel.
        fence(Ordering::SeqCst);
        if self.waiters.load(Ordering::Relaxed) > 0 {
            // A waiter holds the lock from before it checks the channel until it waits
            // in the condvar. Taking it here makes sure the waiter is waiting.
            drop(self.lock());
            self.cvar.notify_all();
        }
    }

    /// Call `attempt` until it returns `Some`, waiting for a notification between tries.
    /// Returns `None` if `deadline` passes first.
    fn wait<R>(
        &self,
        deadline: Option<Instant>,
        mut attempt: impl FnMut() -> Option<R>,
    ) -> Option<R> {
        if let Some(result) = attempt() {
            return Some(result);
        }
        let mut guard = self.lock();
        self.waiters.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::SeqCst);
        let result = loop {
            if let Some(result) = attempt() {
                break Some(result);
            }
            match deadline {
                None => {
                    guard = self
                        .cvar
                        .wait(guard)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break None;
                    }
                    guard = self
                        .cvar
                        .wait_timeout(guard, deadline - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        };
        self.waiters.fetch_sub(1, Ordering::Relaxed);
        drop(guard);
        result
    }
}

#[derive(Default)]
struct Shared {
    /// Notified when a message was sent, or the last sender was dropped.
    sent: Signal,
    /// Notified when a message was received, or the last receiver was dropped.
    received: Signal,
}

/// Create a bounded channel that holds up to `capacity` messages.
///
/// A capacity of zero is raised to one.
/// A zero capacity crossbeam channel only passes a message to a receiver that is blocked in it,
/// and the receivers of this channel never block in the crossbeam channel.
pub fn bounded<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = crossbeam_channel::bounded(capacity.max(1));
    let shared = Arc::new(Shared::default());
    (
        Sender {
            inner: ManuallyDrop::new(tx),
            shared: shared.clone(),
        },
        Receiver {
            inner: ManuallyDrop::new(rx),
            shared,
        },
    )
}

/// The sending side of a channel created by [`bounded`].
pub struct Sender<T> {
    inner: ManuallyDrop<crossbeam_channel::Sender<T>>,
    shared: Arc<Shared>,
}

impl<T> Sender<T> {
    /// Send a message without blocking.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        self.inner.try_send(msg)?;
        self.shared.sent.notify();
        Ok(())
    }

    /// Send a message, waiting for room in the channel if it is full.
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        self.send_deadline(msg, None).map_err(|err| match err {
            SendTimeoutError::Disconnected(msg) | SendTimeoutError::Timeout(msg) => SendError(msg),
        })
    }

    /// Send a message, waiting up to `timeout` for room in the channel if it is full.
    pub fn send_timeout(&self, msg: T, timeout: Duration) -> Result<(), SendTimeoutError<T>> {
        self.send_deadline(msg, Instant::now().checked_add(timeout))
    }

    fn send_deadline(&self, msg: T, deadline: Option<Instant>) -> Result<(), SendTimeoutError<T>> {
        let mut pending = Some(msg);
        let result =
            self.shared
                .received
                .wait(deadline, || match self.inner.try_send(pending.take()?) {
                    Ok(()) => Some(Ok(())),
                    Err(TrySendError::Full(msg)) => {
                        pending = Some(msg);
                        None
                    }
                    Err(TrySendError::Disconnected(msg)) => {
                        Some(Err(SendTimeoutError::Disconnected(msg)))
                    }
                });
        match result {
            Some(Ok(())) => {
                self.shared.sent.notify();
                Ok(())
            }
            Some(Err(err)) => Err(err),
            // `pending` always holds the message when `wait` times out.
            None => Err(SendTimeoutError::Timeout(pending.take().unwrap())),
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            inner: ManuallyDrop::new((*self.inner).clone()),
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // SAFETY: `inner` is not used again after this.
        unsafe { ManuallyDrop::drop(&mut self.inner) };
        // Dropping the last sender disconnects the channel.
        // The receivers only see that when they look again.
        self.shared.sent.notify();
    }
}

/// The receiving side of a channel created by [`bounded`].
pub struct Receiver<T> {
    inner: ManuallyDrop<crossbeam_channel::Receiver<T>>,
    shared: Arc<Shared>,
}

impl<T> Receiver<T> {
    /// Receive a message without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let msg = self.inner.try_recv()?;
        self.shared.received.notify();
        Ok(msg)
    }

    /// Receive a message, waiting for one if the channel is empty.
    pub fn recv(&self) -> Result<T, RecvError> {
        self.recv_deadline(None).map_err(|_| RecvError)
    }

    /// Receive a message, waiting up to `timeout` for one if the channel is empty.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        self.recv_deadline(Instant::now().checked_add(timeout))
    }

    fn recv_deadline(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let result = self
            .shared
            .sent
            .wait(deadline, || match self.inner.try_recv() {
                Ok(msg) => Some(Ok(msg)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(RecvTimeoutError::Disconnected)),
            });
        match result {
            Some(Ok(msg)) => {
                self.shared.received.notify();
                Ok(msg)
            }
            Some(Err(err)) => Err(err),
            None => Err(RecvTimeoutError::Timeout),
        }
    }

    /// The number of messages in the channel.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the channel holds no messages.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // SAFETY: `inner` is not used again after this.
        unsafe { ManuallyDrop::drop(&mut self.inner) };
        // Dropping the receiver disconnects the channel, wake the senders waiting for room.
        self.shared.received.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn send_and_receive() {
        let (tx, rx) = bounded(2);
        tx.send(1).unwrap();
        tx.try_send(2).unwrap();
        assert!(matches!(tx.try_send(3), Err(TrySendError::Full(3))));
        assert_eq!(rx.len(), 2);
        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.try_recv().unwrap(), 2);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn zero_capacity_holds_one() {
        let (tx, rx) = bounded(0);
        tx.try_send(1).unwrap();
        assert!(matches!(tx.try_send(2), Err(TrySendError::Full(2))));
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn timeouts() {
        let (tx, rx) = bounded::<i32>(1);
        let timeout = Duration::from_millis(20);
        let start = Instant::now();
        assert!(matches!(
            rx.recv_timeout(timeout),
            Err(RecvTimeoutError::Timeout)
        ));
        assert!(start.elapsed() >= timeout);
        tx.send(1).unwrap();
        let start = Instant::now();
        assert!(matches!(
            tx.send_timeout(2, timeout),
            Err(SendTimeoutError::Timeout(2))
        ));
        assert!(start.elapsed() >= timeout);
    }

    #[test]
    fn recv_wakes_on_send() {
        let (tx, rx) = bounded(1);
        let handle = thread::spawn(move || rx.recv().unwrap());
        thread::sleep(Duration::from_millis(20));
        tx.send(5).unwrap();
        assert_eq!(handle.join().unwrap(), 5);
    }

    #[test]
    fn send_wakes_on_recv() {
        let (tx, rx) = bounded(1);
        tx.send(1).unwrap();
        let handle = thread::spawn(move || tx.send(2).unwrap());
        thread::sleep(Duration::from_millis(20));
        assert_eq!(rx.recv().unwrap(), 1);
        handle.join().unwrap();
        assert_eq!(rx.recv().unwrap(), 2);
    }

    #[test]
    fn recv_wakes_on_disconnect() {
        let (tx, rx) = bounded::<i32>(1);
        let tx2 = tx.clone();
        let handle = thread::spawn(move || rx.recv());
        thread::sleep(Duration::from_millis(20));
        drop(tx);
        thread::sleep(Duration::from_millis(20));
        assert!(!handle.is_finished());
        drop(tx2);
        assert!(handle.join().unwrap().is_err());
    }

    #[test]
    fn send_wakes_on_disconnect() {
        let (tx, rx) = bounded(1);
        tx.send(1).unwrap();
        let handle = thread::spawn(move || tx.send(2));
        thread::sleep(Duration::from_millis(20));
        drop(rx);
        assert!(matches!(handle.join().unwrap(), Err(SendError(2))));
    }

    #[test]
    fn many_messages_in_order() {
        // Many hand-overs through a short channel, so that both sides wait often.
        const COUNT: usize = 100_000;
        let (tx, rx) = bounded(1);
        let producer = thread::spawn(move || {
            for n in 0..COUNT {
                tx.send(n).unwrap();
            }
        });
        for n in 0..COUNT {
            assert_eq!(rx.recv().unwrap(), n);
        }
        assert!(rx.recv().is_err());
        producer.join().unwrap();
    }
}
