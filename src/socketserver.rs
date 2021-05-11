use clap::crate_version;
#[cfg(feature = "secure-websocket")]
use native_tls::{Identity, TlsAcceptor, TlsStream};
use serde::{Deserialize, Serialize};
#[cfg(feature = "secure-websocket")]
use std::fs::File;
#[cfg(feature = "secure-websocket")]
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use tungstenite::server::accept;
use tungstenite::Message;
use tungstenite::WebSocket;

use crate::{CaptureStatus, PlaybackStatus, ProcessingStatus};
use config;
use ExitRequest;
use ProcessingState;
use Res;

#[derive(Debug, Clone)]
pub struct SharedData {
    pub signal_reload: Arc<AtomicBool>,
    pub signal_exit: Arc<AtomicUsize>,
    pub active_config: Arc<Mutex<Option<config::Configuration>>>,
    pub active_config_path: Arc<Mutex<Option<String>>>,
    pub new_config: Arc<Mutex<Option<config::Configuration>>>,
    pub capture_status: Arc<RwLock<CaptureStatus>>,
    pub playback_status: Arc<RwLock<PlaybackStatus>>,
    pub processing_status: Arc<RwLock<ProcessingStatus>>,
}

#[derive(Debug, Clone)]
pub struct ServerParameters<'a> {
    pub address: &'a str,
    pub port: usize,
    #[cfg(feature = "secure-websocket")]
    pub cert_file: Option<&'a str>,
    #[cfg(feature = "secure-websocket")]
    pub cert_pass: Option<&'a str>,
}

fn list_supported_devices() -> (Vec<String>, Vec<String>) {
    let mut playbacktypes = vec!["File".to_owned(), "Stdout".to_owned()];
    let mut capturetypes = vec!["File".to_owned(), "Stdin".to_owned()];

    if cfg!(feature = "alsa-backend") {
        playbacktypes.push("Alsa".to_owned());
        capturetypes.push("Alsa".to_owned());
    }
    if cfg!(feature = "pulse-backend") {
        playbacktypes.push("Pulse".to_owned());
        capturetypes.push("Pulse".to_owned());
    }
    if cfg!(feature = "jack-backend") {
        playbacktypes.push("Jack".to_owned());
        capturetypes.push("Jack".to_owned());
    }
    if cfg!(all(feature = "cpal-backend", target_os = "macos")) {
        playbacktypes.push("CoreAudio".to_owned());
        capturetypes.push("CoreAudio".to_owned());
    }
    if cfg!(all(feature = "cpal-backend", target_os = "windows")) {
        playbacktypes.push("Wasapi".to_owned());
        capturetypes.push("Wasapi".to_owned());
    }
    (playbacktypes, capturetypes)
}

#[derive(Debug, PartialEq, Deserialize)]
enum WsCommand {
    SetConfigName(String),
    SetConfig(String),
    SetConfigJson(String),
    Reload,
    GetConfig,
    ReadConfig(String),
    ReadConfigFile(String),
    ValidateConfig(String),
    GetConfigJson,
    GetConfigName,
    GetSignalRange,
    GetCaptureSignalRms,
    GetCaptureSignalPeak,
    GetPlaybackSignalRms,
    GetPlaybackSignalPeak,
    GetCaptureRate,
    GetUpdateInterval,
    SetUpdateInterval(usize),
    GetVolume,
    SetVolume(f32),
    GetMute,
    SetMute(bool),
    GetVersion,
    GetState,
    GetRateAdjust,
    GetClippedSamples,
    GetBufferLevel,
    GetSupportedDeviceTypes,
    Exit,
    Stop,
    None,
}

#[derive(Debug, PartialEq, Serialize)]
enum WsResult {
    Ok,
    Error,
}

#[derive(Debug, PartialEq, Serialize)]
enum WsReply {
    SetConfigName {
        result: WsResult,
    },
    SetConfig {
        result: WsResult,
    },
    SetConfigJson {
        result: WsResult,
    },
    Reload {
        result: WsResult,
    },
    GetConfig {
        result: WsResult,
        value: String,
    },
    ReadConfig {
        result: WsResult,
        value: String,
    },
    ReadConfigFile {
        result: WsResult,
        value: String,
    },
    ValidateConfig {
        result: WsResult,
        value: String,
    },
    GetConfigJson {
        result: WsResult,
        value: String,
    },
    GetConfigName {
        result: WsResult,
        value: String,
    },
    GetSignalRange {
        result: WsResult,
        value: f32,
    },
    GetPlaybackSignalRms {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeak {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRms {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeak {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureRate {
        result: WsResult,
        value: usize,
    },
    GetUpdateInterval {
        result: WsResult,
        value: usize,
    },
    SetUpdateInterval {
        result: WsResult,
    },
    SetVolume {
        result: WsResult,
    },
    GetVolume {
        result: WsResult,
        value: f32,
    },
    SetMute {
        result: WsResult,
    },
    GetMute {
        result: WsResult,
        value: bool,
    },
    GetVersion {
        result: WsResult,
        value: String,
    },
    GetState {
        result: WsResult,
        value: ProcessingState,
    },
    GetRateAdjust {
        result: WsResult,
        value: f32,
    },
    GetBufferLevel {
        result: WsResult,
        value: usize,
    },
    GetClippedSamples {
        result: WsResult,
        value: usize,
    },
    GetSupportedDeviceTypes {
        result: WsResult,
        value: (Vec<String>, Vec<String>),
    },
    Exit {
        result: WsResult,
    },
    Stop {
        result: WsResult,
    },
    Invalid {
        error: String,
    },
}

fn parse_command(cmd: Message) -> Res<WsCommand> {
    match cmd {
        Message::Text(command_str) => {
            let command = serde_json::from_str::<WsCommand>(&command_str)?;
            Ok(command)
        }
        _ => Ok(WsCommand::None),
    }
}

#[cfg(feature = "secure-websocket")]
fn make_acceptor_with_cert(cert: &str, key: &str) -> Res<Arc<TlsAcceptor>> {
    let mut file = File::open(cert)?;
    let mut identity = vec![];
    file.read_to_end(&mut identity)?;
    let identity = Identity::from_pkcs12(&identity, key)?;
    let acceptor = TlsAcceptor::new(identity)?;
    Ok(Arc::new(acceptor))
}

#[cfg(feature = "secure-websocket")]
fn make_acceptor(cert_file: &Option<&str>, cert_key: &Option<&str>) -> Option<Arc<TlsAcceptor>> {
    if let (Some(cert), Some(key)) = (cert_file, cert_key) {
        let acceptor = make_acceptor_with_cert(&cert, &key);
        match acceptor {
            Ok(acc) => {
                debug!("Created TLS acceptor");
                return Some(acc);
            }
            Err(err) => {
                error!("Could not create TLS acceptor: {}", err);
            }
        }
    }
    debug!("Running websocket server without TLS");
    None
}

pub fn start_server(parameters: ServerParameters, shared_data: SharedData) {
    let address = parameters.address.to_string();
    let port = parameters.port;
    debug!("Start websocket server on {}:{}", address, parameters.port);
    #[cfg(feature = "secure-websocket")]
    let acceptor = make_acceptor(&parameters.cert_file, &parameters.cert_pass);

    thread::spawn(move || {
        let ws_result = TcpListener::bind(format!("{}:{}", address, port));
        if let Ok(server) = ws_result {
            for stream in server.incoming() {
                let shared_data_inst = shared_data.clone();
                #[cfg(feature = "secure-websocket")]
                let acceptor_inst = acceptor.clone();

                #[cfg(feature = "secure-websocket")]
                thread::spawn(move || match acceptor_inst {
                    None => {
                        let websocket_res = accept_plain_stream(stream);
                        handle_tcp(websocket_res, &shared_data_inst);
                    }
                    Some(acc) => {
                        let websocket_res = accept_secure_stream(acc, stream);
                        handle_tls(websocket_res, &shared_data_inst);
                    }
                });
                #[cfg(not(feature = "secure-websocket"))]
                thread::spawn(move || {
                    let websocket_res = accept_plain_stream(stream);
                    handle_tcp(websocket_res, &shared_data_inst);
                });
            }
        } else if let Err(err) = ws_result {
            error!("Failed to start websocket server: {}", err);
        }
    });
}

macro_rules! make_handler {
    ($t:ty, $n:ident) => {
        fn $n(websocket_res: Res<WebSocket<$t>>, shared_data_inst: &SharedData) {
            match websocket_res {
                Ok(mut websocket) => loop {
                    let msg_res = websocket.read_message();
                    match msg_res {
                        Ok(msg) => {
                            trace!("received: {:?}", msg);
                            let command = parse_command(msg);
                            debug!("parsed command: {:?}", command);
                            let reply = match command {
                                Ok(cmd) => handle_command(cmd, &shared_data_inst),
                                Err(err) => Some(WsReply::Invalid {
                                    error: format!("{}", err).to_string(),
                                }),
                            };
                            if let Some(rep) = reply {
                                let write_result = websocket.write_message(Message::text(
                                    serde_json::to_string(&rep).unwrap(),
                                ));
                                if let Err(err) = write_result {
                                    warn!("Failed to write: {}", err);
                                    break;
                                }
                            } else {
                                debug!("Sending no reply");
                            }
                        }
                        Err(tungstenite::error::Error::ConnectionClosed) => {
                            debug!("Connection was closed");
                            break;
                        }
                        Err(err) => {
                            warn!("Lost connection: {}", err);
                            break;
                        }
                    }
                },
                Err(err) => warn!("Connection failed: {}", err),
            };
        }
    };
}

make_handler!(TcpStream, handle_tcp);
#[cfg(feature = "secure-websocket")]
make_handler!(TlsStream<TcpStream>, handle_tls);

#[cfg(feature = "secure-websocket")]
fn accept_secure_stream(
    acceptor: Arc<TlsAcceptor>,
    stream: Result<TcpStream, std::io::Error>,
) -> Res<tungstenite::WebSocket<TlsStream<TcpStream>>> {
    let ws = accept(acceptor.accept(stream?)?)?;
    Ok(ws)
}

fn accept_plain_stream(
    stream: Result<TcpStream, std::io::Error>,
) -> Res<tungstenite::WebSocket<TcpStream>> {
    let ws = accept(stream?)?;
    Ok(ws)
}

fn handle_command(command: WsCommand, shared_data_inst: &SharedData) -> Option<WsReply> {
    match command {
        WsCommand::Reload => {
            shared_data_inst
                .signal_reload
                .store(true, Ordering::Relaxed);
            Some(WsReply::Reload {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetCaptureRate => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetCaptureRate {
                result: WsResult::Ok,
                value: capstat.measured_samplerate,
            })
        }
        WsCommand::GetSignalRange => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetSignalRange {
                result: WsResult::Ok,
                value: capstat.signal_range,
            })
        }
        WsCommand::GetCaptureSignalRms => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetCaptureSignalRms {
                result: WsResult::Ok,
                value: capstat.signal_rms.clone(),
            })
        }
        WsCommand::GetPlaybackSignalRms => {
            let pbstat = shared_data_inst.playback_status.read().unwrap();
            Some(WsReply::GetPlaybackSignalRms {
                result: WsResult::Ok,
                value: pbstat.signal_rms.clone(),
            })
        }
        WsCommand::GetCaptureSignalPeak => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetCaptureSignalPeak {
                result: WsResult::Ok,
                value: capstat.signal_peak.clone(),
            })
        }
        WsCommand::GetPlaybackSignalPeak => {
            let pbstat = shared_data_inst.playback_status.read().unwrap();
            Some(WsReply::GetPlaybackSignalPeak {
                result: WsResult::Ok,
                value: pbstat.signal_peak.clone(),
            })
        }
        WsCommand::GetVersion => Some(WsReply::GetVersion {
            result: WsResult::Ok,
            value: crate_version!().to_string(),
        }),
        WsCommand::GetState => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetState {
                result: WsResult::Ok,
                value: capstat.state,
            })
        }
        WsCommand::GetRateAdjust => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetRateAdjust {
                result: WsResult::Ok,
                value: capstat.rate_adjust,
            })
        }
        WsCommand::GetClippedSamples => {
            let pbstat = shared_data_inst.playback_status.read().unwrap();
            Some(WsReply::GetClippedSamples {
                result: WsResult::Ok,
                value: pbstat.clipped_samples,
            })
        }
        WsCommand::GetBufferLevel => {
            let pbstat = shared_data_inst.playback_status.read().unwrap();
            Some(WsReply::GetBufferLevel {
                result: WsResult::Ok,
                value: pbstat.buffer_level,
            })
        }
        WsCommand::GetUpdateInterval => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WsReply::GetUpdateInterval {
                result: WsResult::Ok,
                value: capstat.update_interval,
            })
        }
        WsCommand::SetUpdateInterval(nbr) => {
            shared_data_inst
                .capture_status
                .write()
                .unwrap()
                .update_interval = nbr;
            shared_data_inst
                .playback_status
                .write()
                .unwrap()
                .update_interval = nbr;
            Some(WsReply::SetUpdateInterval {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetVolume => {
            let procstat = shared_data_inst.processing_status.read().unwrap();
            Some(WsReply::GetVolume {
                result: WsResult::Ok,
                value: procstat.volume,
            })
        }
        WsCommand::SetVolume(nbr) => {
            let mut procstat = shared_data_inst.processing_status.write().unwrap();
            procstat.volume = nbr;
            Some(WsReply::SetVolume {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetMute => {
            let procstat = shared_data_inst.processing_status.read().unwrap();
            Some(WsReply::GetMute {
                result: WsResult::Ok,
                value: procstat.mute,
            })
        }
        WsCommand::SetMute(mute) => {
            let mut procstat = shared_data_inst.processing_status.write().unwrap();
            procstat.mute = mute;
            Some(WsReply::SetMute {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetConfig => Some(WsReply::GetConfig {
            result: WsResult::Ok,
            value: serde_yaml::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        }),
        WsCommand::GetConfigJson => Some(WsReply::GetConfigJson {
            result: WsResult::Ok,
            value: serde_json::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        }),
        WsCommand::GetConfigName => Some(WsReply::GetConfigName {
            result: WsResult::Ok,
            value: shared_data_inst
                .active_config_path
                .lock()
                .unwrap()
                .as_ref()
                .unwrap_or(&"NONE".to_string())
                .to_string(),
        }),
        WsCommand::SetConfigName(path) => match config::load_validate_config(&path) {
            Ok(_) => {
                *shared_data_inst.active_config_path.lock().unwrap() = Some(path.clone());
                Some(WsReply::SetConfigName {
                    result: WsResult::Ok,
                })
            }
            Err(error) => {
                error!("Error setting config name: {}", error);
                Some(WsReply::SetConfigName {
                    result: WsResult::Error,
                })
            }
        },
        WsCommand::SetConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        Some(WsReply::SetConfig {
                            result: WsResult::Ok,
                        })
                    }
                    Err(error) => {
                        error!("Error setting config: {}", error);
                        Some(WsReply::SetConfig {
                            result: WsResult::Error,
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WsReply::SetConfig {
                        result: WsResult::Error,
                    })
                }
            }
        }
        WsCommand::SetConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        Some(WsReply::SetConfigJson {
                            result: WsResult::Ok,
                        })
                    }
                    Err(error) => {
                        error!("Error setting config: {}", error);
                        Some(WsReply::SetConfigJson {
                            result: WsResult::Error,
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WsReply::SetConfigJson {
                        result: WsResult::Error,
                    })
                }
            }
        }
        WsCommand::ReadConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => Some(WsReply::ReadConfig {
                    result: WsResult::Ok,
                    value: serde_yaml::to_string(&conf).unwrap(),
                }),
                Err(error) => {
                    error!("Error reading config: {}", error);
                    Some(WsReply::ReadConfig {
                        result: WsResult::Error,
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::ReadConfigFile(path) => match config::load_config(&path) {
            Ok(conf) => Some(WsReply::ReadConfigFile {
                result: WsResult::Ok,
                value: serde_yaml::to_string(&conf).unwrap(),
            }),
            Err(error) => {
                error!("Error reading config file: {}", error);
                Some(WsReply::ReadConfigFile {
                    result: WsResult::Error,
                    value: error.to_string(),
                })
            }
        },
        WsCommand::ValidateConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => Some(WsReply::ValidateConfig {
                        result: WsResult::Ok,
                        value: serde_yaml::to_string(&conf).unwrap(),
                    }),
                    Err(error) => {
                        error!("Config error: {}", error);
                        Some(WsReply::ValidateConfig {
                            result: WsResult::Error,
                            value: error.to_string(),
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WsReply::ValidateConfig {
                        result: WsResult::Error,
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::Stop => {
            *shared_data_inst.new_config.lock().unwrap() = None;
            shared_data_inst
                .signal_exit
                .store(ExitRequest::STOP, Ordering::Relaxed);
            Some(WsReply::Stop {
                result: WsResult::Ok,
            })
        }
        WsCommand::Exit => {
            shared_data_inst
                .signal_exit
                .store(ExitRequest::EXIT, Ordering::Relaxed);
            Some(WsReply::Exit {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetSupportedDeviceTypes => {
            let devs = list_supported_devices();
            Some(WsReply::GetSupportedDeviceTypes {
                result: WsResult::Ok,
                value: devs,
            })
        }
        WsCommand::None => None,
    }
}

#[cfg(test)]
mod tests {
    use socketserver::{parse_command, WsCommand};
    use tungstenite::Message;

    #[test]
    fn parse_commands() {
        let cmd = Message::text("\"Reload\"");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::Reload);
        let cmd = Message::text("asdfasdf");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("{\"SetConfigName\": \"somefile\"}");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::SetConfigName("somefile".to_string()));
    }
}
