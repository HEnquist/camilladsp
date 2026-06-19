#[cfg(not(windows))]
mod unix_signals;
#[cfg(windows)]
mod windows_signals;

const EXIT_FORCED: i32 = 103; // Exit was forced by a second SIGINT

#[cfg(not(windows))]
pub use unix_signals::handle_signals;

#[cfg(windows)]
pub use windows_signals::handle_signals;
