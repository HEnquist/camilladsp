use std::{sync::{Arc, atomic::AtomicBool}, thread, time::Duration};

use crate::{ControllerMessage, signals::EXIT_FORCED};

pub fn handle_signals(logger: flexi_logger::LoggerHandle, tx_command_thread: crossbeam_channel::Sender<ControllerMessage>) {
    thread::Builder::new()
        .name("signals".to_string())
        .spawn(move || {
        // On windows we don't have signal_hook::iterator, so we just poll for signal...
        const DELAY: Duration = Duration::from_millis(100);
        let signal_exit = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&signal_exit)).unwrap();
        let mut exit_requested = false;
        loop {
            if signal_exit.load(std::sync::atomic::Ordering::Relaxed) {
                signal_exit.store(false, std::sync::atomic::Ordering::Relaxed);
                if exit_requested {
                    warn!("Forcing a shutdown");
                    logger.flush();
                    std::process::exit(EXIT_FORCED);
                }
                info!("Shutting down");
                exit_requested = true;
                if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                    error!("Error sending exit message: {e}");
                }
            }
            thread::sleep(DELAY);
        }
    }).expect("can spawn signals thread");
}
