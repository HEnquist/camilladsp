use std::{sync::Arc, thread};

use signal_hook::consts::{SIGHUP, SIGUSR1};
use signal_hook::{
    consts::TERM_SIGNALS,
    iterator::{SignalsInfo, exfiltrator::SignalOnly},
};

use crate::{ControllerMessage, config, signals::EXIT_FORCED};

pub fn handle_signals(
    logger: flexi_logger::LoggerHandle,
    tx_command_thread: crossbeam_channel::Sender<ControllerMessage>,
    active_path_thread: Arc<parking_lot::lock_api::Mutex<parking_lot::RawMutex, Option<String>>>,
) {
    thread::Builder::new()
        .name("signals".to_string())
        .spawn(move || {
            let mut sigs = vec![SIGHUP, SIGUSR1];
            sigs.extend(TERM_SIGNALS);
            let mut signals = SignalsInfo::<SignalOnly>::new(&sigs).unwrap();
            let mut exit_requested = false;
            for info in &mut signals {
                debug!("Received signal: {info}");
                match info {
                    SIGHUP => {
                        let path = (*active_path_thread.lock()).clone();
                        if let Some(path) = path {
                            match config::load_validate_config(path.as_str()) {
                                Ok(conf) => {
                                    debug!("Config is valid");
                                    if let Err(e) = tx_command_thread
                                        .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                                    {
                                        error!("Error sending reload message: {e}");
                                    }
                                }
                                Err(err) => {
                                    error!("Config error during reload: {err}");
                                }
                            };
                        } else {
                            error!("Config path not specified, cannot reload");
                        }
                    }
                    SIGUSR1 => {
                        if let Err(e) = tx_command_thread.try_send(ControllerMessage::Stop) {
                            error!("Error sending stop message: {e}");
                        }
                    }
                    _ => {
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
                };
            }
        })
        .expect("can spawn signals thread");
}
