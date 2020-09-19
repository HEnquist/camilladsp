use clap::crate_version;
use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use tungstenite::server::accept;
use tungstenite::Message;

use crate::CaptureStatus;
use config;
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
    Exit,
    Stop,
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
    let command_str = cmd.into_text()?;
    let command = serde_json::from_str::<WSCommand>(&command_str)?;
    return Ok(command);
}

pub fn start_server(bind_address: &str, port: usize, shared_data: SharedData) {
    let address = bind_address.to_owned();
    debug!("Start websocket server on port {}", port);

    thread::spawn(move || {
        let ws_result = TcpListener::bind(format!("{}:{}", address, port));
        if let Ok(server) = ws_result {
            for stream in server.incoming() {
                let shared_data_inst = shared_data.clone();
                thread::spawn(move || {
                    let mut websocket = accept(stream.unwrap()).unwrap();
                    loop {
                        let msg_res = websocket.read_message();
                        match msg_res {
                            Ok(msg) => {
                                let command = parse_command(msg);
                                debug!("parsed command: {:?}", command);
                                let reply = match command {
                                    Ok(cmd) => handle_command(cmd, &shared_data_inst),
                                    Err(err) => WSReply::Invalid {
                                        error: format!("{}", err).to_string(),
                                    },
                                };
                                let write_result = websocket.write_message(Message::text(
                                    serde_json::to_string(&reply).unwrap(),
                                ));
                                if let Err(err) = write_result {
                                    warn!("Failed to write: {}", err);
                                    break;
                                }
                            }
                            Err(err) => {
                                warn!("Lost connection: {}", err);
                                break;
                            }
                        }
                    }
                });
            }
        } else if let Err(err) = ws_result {
            error!("Failed to start websocket server: {}", err);
        }
    });
}

fn handle_command(command: WSCommand, shared_data_inst: &SharedData) -> WSReply {
    match command {
        WSCommand::Reload => {
            shared_data_inst
                .signal_reload
                .store(true, Ordering::Relaxed);
            WSReply::Reload {
                result: WSResult::Ok,
            }
        }
        WSCommand::GetCaptureRate => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            WSReply::GetCaptureRate {
                result: WSResult::Ok,
                value: capstat.measured_samplerate,
            }
        }
        WSCommand::GetSignalRange => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            WSReply::GetSignalRange {
                result: WSResult::Ok,
                value: capstat.signal_range,
            }
        }
        WSCommand::GetVersion => WSReply::GetVersion {
            result: WSResult::Ok,
            value: crate_version!().to_string(),
        },
        WSCommand::GetState => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            WSReply::GetState {
                result: WSResult::Ok,
                value: capstat.state.clone(),
            }
        }
        WSCommand::GetRateAdjust => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            WSReply::GetRateAdjust {
                result: WSResult::Ok,
                value: capstat.rate_adjust,
            }
        }
        WSCommand::GetUpdateInterval => {
            let capstat = shared_data_inst.capture_status.read().unwrap();
            WSReply::GetUpdateInterval {
                result: WSResult::Ok,
                value: capstat.update_interval,
            }
        }
        WSCommand::SetUpdateInterval(nbr) => {
            let mut capstat = shared_data_inst.capture_status.write().unwrap();
            capstat.update_interval = nbr;
            WSReply::SetUpdateInterval {
                result: WSResult::Ok,
            }
        }
        WSCommand::GetConfig => WSReply::GetConfig {
            result: WSResult::Ok,
            value: serde_yaml::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        },
        WSCommand::GetConfigJson => WSReply::GetConfigJson {
            result: WSResult::Ok,
            value: serde_json::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        },
        WSCommand::GetConfigName => WSReply::GetConfigName {
            result: WSResult::Ok,
            value: shared_data_inst
                .active_config_path
                .lock()
                .unwrap()
                .as_ref()
                .unwrap_or(&"NONE".to_string())
                .to_string(),
        },
        WSCommand::SetConfigName(path) => match config::load_validate_config(&path) {
            Ok(_) => {
                *shared_data_inst.active_config_path.lock().unwrap() = Some(path.clone());
                WSReply::SetConfigName {
                    result: WSResult::Ok,
                }
            }
            _ => WSReply::SetConfigName {
                result: WSResult::Error,
            },
        },
        WSCommand::SetConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => match config::validate_config(conf.clone()) {
                    Ok(()) => {
                        //*active_config_path_inst.lock().unwrap() = String::from("none");
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        WSReply::SetConfig {
                            result: WSResult::Ok,
                        }
                    }
                    _ => WSReply::SetConfig {
                        result: WSResult::Error,
                    },
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    WSReply::SetConfig {
                        result: WSResult::Error,
                    }
                }
            }
        }
        WSCommand::SetConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(conf) => match config::validate_config(conf.clone()) {
                    Ok(()) => {
                        //*active_config_path_inst.lock().unwrap() = String::from("none");
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        WSReply::SetConfigJson {
                            result: WSResult::Ok,
                        }
                    }
                    _ => WSReply::SetConfigJson {
                        result: WSResult::Error,
                    },
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    WSReply::SetConfigJson {
                        result: WSResult::Error,
                    }
                }
            }
        }
        WSCommand::ReadConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => WSReply::ReadConfig {
                    result: WSResult::Ok,
                    value: format!("{}", serde_yaml::to_string(&conf).unwrap()),
                },
                Err(error) => WSReply::ReadConfig {
                    result: WSResult::Error,
                    value: format!("{}", error),
                },
            }
        }
        WSCommand::ReadConfigFile(path) => match config::load_config(&path) {
            Ok(conf) => WSReply::ReadConfigFile {
                result: WSResult::Ok,
                value: format!("{}", serde_yaml::to_string(&conf).unwrap()),
            },
            Err(error) => WSReply::ReadConfigFile {
                result: WSResult::Error,
                value: format!("{}", error),
            },
        },
        WSCommand::ValidateConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => match config::validate_config(conf.clone()) {
                    Ok(()) => WSReply::ValidateConfig {
                        result: WSResult::Ok,
                        value: format!("{}", serde_yaml::to_string(&conf).unwrap()),
                    },
                    Err(error) => WSReply::ValidateConfig {
                        result: WSResult::Error,
                        value: format!("{}", error),
                    },
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    WSReply::ValidateConfig {
                        result: WSResult::Error,
                        value: format!("{}", error),
                    }
                }
            }
        }
        WSCommand::Stop => {
            *shared_data_inst.new_config.lock().unwrap() = None;
            shared_data_inst.signal_exit.store(2, Ordering::Relaxed);
            WSReply::Stop {
                result: WSResult::Ok,
            }
        }
        WSCommand::Exit => {
            shared_data_inst.signal_exit.store(1, Ordering::Relaxed);
            WSReply::Exit {
                result: WSResult::Ok,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use socketserver::{parse_command, WSCommand};
    use ws::Message;

    #[test]
    fn parse_commands() {
        let cmd = Message::text("reload");
        let res = parse_command(&cmd);
        assert_eq!(res, WSCommand::Reload);
        let cmd = Message::text("asdfasdf");
        let res = parse_command(&cmd);
        assert_eq!(res, WSCommand::Invalid);
        let cmd = Message::text("");
        let res = parse_command(&cmd);
        assert_eq!(res, WSCommand::Invalid);
        let cmd = Message::text("setconfigname:somefile");
        let res = parse_command(&cmd);
        assert_eq!(res, WSCommand::SetConfigName("somefile".to_string()));
    }
}
