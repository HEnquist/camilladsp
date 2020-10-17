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

use crate::{CaptureStatus, PlaybackStatus};
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

#[derive(Debug, PartialEq, Deserialize)]
enum WSCommand {
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
    GetCaptureRate,
    GetUpdateInterval,
    SetUpdateInterval(usize),
    GetVersion,
    GetState,
    GetRateAdjust,
    GetClippedSamples,
    GetBufferLevel,
    Exit,
    Stop,
    None,
}

#[derive(Debug, PartialEq, Serialize)]
enum WSResult {
    Ok,
    Error,
}

#[derive(Debug, PartialEq, Serialize)]
enum WSReply {
    SetConfigName {
        result: WSResult,
    },
    SetConfig {
        result: WSResult,
    },
    SetConfigJson {
        result: WSResult,
    },
    Reload {
        result: WSResult,
    },
    GetConfig {
        result: WSResult,
        value: String,
    },
    ReadConfig {
        result: WSResult,
        value: String,
    },
    ReadConfigFile {
        result: WSResult,
        value: String,
    },
    ValidateConfig {
        result: WSResult,
        value: String,
    },
    GetConfigJson {
        result: WSResult,
        value: String,
    },
    GetConfigName {
        result: WSResult,
        value: String,
    },
    GetSignalRange {
        result: WSResult,
        value: f32,
    },
    GetCaptureRate {
        result: WSResult,
        value: usize,
    },
    GetUpdateInterval {
        result: WSResult,
        value: usize,
    },
    SetUpdateInterval {
        result: WSResult,
    },
    GetVersion {
        result: WSResult,
        value: String,
    },
    GetState {
        result: WSResult,
        value: ProcessingState,
    },
    GetRateAdjust {
        result: WSResult,
        value: f32,
    },
    GetBufferLevel {
        result: WSResult,
        value: usize,
    },
    GetClippedSamples {
        result: WSResult,
        value: usize,
    },
    Exit {
        result: WSResult,
    },
    Stop {
        result: WSResult,
    },
    Invalid {
        error: String,
    },
}

fn parse_command(cmd: Message) -> Res<WSCommand> {
    match cmd {
        Message::Text(command_str) => {
            let command = serde_json::from_str::<WSCommand>(&command_str)?;
            Ok(command)
        }
        _ => Ok(WSCommand::None),
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
                                Err(err) => Some(WSReply::Invalid {
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

fn handle_command(command: WSCommand, shared_data_inst: &SharedData) -> Option<WSReply> {
    match command {
        WSCommand::Reload => {
            shared_data_inst
                .signal_reload
                .store(true, Ordering::Relaxed);
            Some(WSReply::Reload {
                result: WSResult::Ok,
            })
        }
        WSCommand::GetCaptureRate => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WSReply::GetCaptureRate {
                result: WSResult::Ok,
                value: capstat.measured_samplerate,
            })
        }
        WSCommand::GetSignalRange => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WSReply::GetSignalRange {
                result: WSResult::Ok,
                value: capstat.signal_range,
            })
        }
        WSCommand::GetVersion => Some(WSReply::GetVersion {
            result: WSResult::Ok,
            value: crate_version!().to_string(),
        }),
        WSCommand::GetState => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WSReply::GetState {
                result: WSResult::Ok,
                value: capstat.state,
            })
        }
        WSCommand::GetRateAdjust => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WSReply::GetRateAdjust {
                result: WSResult::Ok,
                value: capstat.rate_adjust,
            })
        }
        WSCommand::GetClippedSamples => {
            let pbstat = shared_data_inst.playback_status.read().unwrap();
            Some(WSReply::GetClippedSamples {
                result: WSResult::Ok,
                value: pbstat.clipped_samples,
            })
        }
        WSCommand::GetBufferLevel => {
            let pbstat = shared_data_inst.playback_status.read().unwrap();
            Some(WSReply::GetBufferLevel {
                result: WSResult::Ok,
                value: pbstat.buffer_level,
            })
        }
        WSCommand::GetUpdateInterval => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            Some(WSReply::GetUpdateInterval {
                result: WSResult::Ok,
                value: capstat.update_interval,
            })
        }
        WSCommand::SetUpdateInterval(nbr) => {
            let mut capstat = shared_data_inst.capture_status.write().unwrap();
            capstat.update_interval = nbr;
            Some(WSReply::SetUpdateInterval {
                result: WSResult::Ok,
            })
        }
        WSCommand::GetConfig => Some(WSReply::GetConfig {
            result: WSResult::Ok,
            value: serde_yaml::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        }),
        WSCommand::GetConfigJson => Some(WSReply::GetConfigJson {
            result: WSResult::Ok,
            value: serde_json::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        }),
        WSCommand::GetConfigName => Some(WSReply::GetConfigName {
            result: WSResult::Ok,
            value: shared_data_inst
                .active_config_path
                .lock()
                .unwrap()
                .as_ref()
                .unwrap_or(&"NONE".to_string())
                .to_string(),
        }),
        WSCommand::SetConfigName(path) => match config::load_validate_config(&path) {
            Ok(_) => {
                *shared_data_inst.active_config_path.lock().unwrap() = Some(path.clone());
                Some(WSReply::SetConfigName {
                    result: WSResult::Ok,
                })
            }
            Err(error) => {
                error!("Error setting config name: {}", error);
                Some(WSReply::SetConfigName {
                    result: WSResult::Error,
                })
            }
        },
        WSCommand::SetConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => match config::validate_config(conf.clone()) {
                    Ok(()) => {
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        Some(WSReply::SetConfig {
                            result: WSResult::Ok,
                        })
                    }
                    Err(error) => {
                        error!("Error setting config: {}", error);
                        Some(WSReply::SetConfig {
                            result: WSResult::Error,
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WSReply::SetConfig {
                        result: WSResult::Error,
                    })
                }
            }
        }
        WSCommand::SetConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(conf) => match config::validate_config(conf.clone()) {
                    Ok(()) => {
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        Some(WSReply::SetConfigJson {
                            result: WSResult::Ok,
                        })
                    }
                    Err(error) => {
                        error!("Error setting config: {}", error);
                        Some(WSReply::SetConfigJson {
                            result: WSResult::Error,
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WSReply::SetConfigJson {
                        result: WSResult::Error,
                    })
                }
            }
        }
        WSCommand::ReadConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => Some(WSReply::ReadConfig {
                    result: WSResult::Ok,
                    value: serde_yaml::to_string(&conf).unwrap(),
                }),
                Err(error) => {
                    error!("Error reading config: {}", error);
                    Some(WSReply::ReadConfig {
                        result: WSResult::Error,
                        value: error.to_string(),
                    })
                }
            }
        }
        WSCommand::ReadConfigFile(path) => match config::load_config(&path) {
            Ok(conf) => Some(WSReply::ReadConfigFile {
                result: WSResult::Ok,
                value: serde_yaml::to_string(&conf).unwrap(),
            }),
            Err(error) => {
                error!("Error reading config file: {}", error);
                Some(WSReply::ReadConfigFile {
                    result: WSResult::Error,
                    value: error.to_string(),
                })
            }
        },
        WSCommand::ValidateConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => match config::validate_config(conf.clone()) {
                    Ok(()) => Some(WSReply::ValidateConfig {
                        result: WSResult::Ok,
                        value: serde_yaml::to_string(&conf).unwrap(),
                    }),
                    Err(error) => {
                        error!("Config error: {}", error);
                        Some(WSReply::ValidateConfig {
                            result: WSResult::Error,
                            value: error.to_string(),
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WSReply::ValidateConfig {
                        result: WSResult::Error,
                        value: error.to_string(),
                    })
                }
            }
        }
        WSCommand::Stop => {
            *shared_data_inst.new_config.lock().unwrap() = None;
            shared_data_inst
                .signal_exit
                .store(ExitRequest::STOP, Ordering::Relaxed);
            Some(WSReply::Stop {
                result: WSResult::Ok,
            })
        }
        WSCommand::Exit => {
            shared_data_inst
                .signal_exit
                .store(ExitRequest::EXIT, Ordering::Relaxed);
            Some(WSReply::Exit {
                result: WSResult::Ok,
            })
        }
        WSCommand::None => None,
    }
}

#[cfg(test)]
mod tests {
    use socketserver::{parse_command, WSCommand};
    use tungstenite::Message;

    #[test]
    fn parse_commands() {
        let cmd = Message::text("\"Reload\"");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WSCommand::Reload);
        let cmd = Message::text("asdfasdf");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("{\"SetConfigName\": \"somefile\"}");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WSCommand::SetConfigName("somefile".to_string()));
    }
}
