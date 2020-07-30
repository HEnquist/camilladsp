use clap::crate_version;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use crate::CaptureStatus;
use config;

#[derive(Debug, PartialEq)]
enum WSCommand {
    SetConfigName(String),
    SetConfig(String),
    SetConfigJson(String),
    Reload,
    GetConfig,
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
    Invalid,
}

fn parse_command(cmd: &ws::Message) -> WSCommand {
    if let Ok(command) = cmd.as_text() {
        let cmdarg: Vec<&str> = command.splitn(2, ':').collect();
        if cmdarg.is_empty() {
            return WSCommand::Invalid;
        }
        debug!("Received: {}", cmdarg[0]);
        match cmdarg[0] {
            "reload" => WSCommand::Reload,
            "getconfig" => WSCommand::GetConfig,
            "getconfigjson" => WSCommand::GetConfigJson,
            "getconfigname" => WSCommand::GetConfigName,
            "exit" => WSCommand::Exit,
            "stop" => WSCommand::Stop,
            "getcapturerate" => WSCommand::GetCaptureRate,
            "getsignalrange" => WSCommand::GetSignalRange,
            "getstate" => WSCommand::GetState,
            "getversion" => WSCommand::GetVersion,
            "getrateadjust" => WSCommand::GetRateAdjust,
            "getupdateinterval" => WSCommand::GetUpdateInterval,
            "setupdateinterval" => {
                if cmdarg.len() == 2 {
                    let nbr_conv = cmdarg[1].to_string().parse::<usize>();
                    match nbr_conv {
                        Ok(nbr) => WSCommand::SetUpdateInterval(nbr),
                        Err(_) => WSCommand::Invalid,
                    }
                } else {
                    WSCommand::Invalid
                }
            }
            "setconfigname" => {
                if cmdarg.len() == 2 {
                    WSCommand::SetConfigName(cmdarg[1].to_string())
                } else {
                    WSCommand::Invalid
                }
            }
            "setconfig" => {
                if cmdarg.len() == 2 {
                    WSCommand::SetConfig(cmdarg[1].to_string())
                } else {
                    WSCommand::Invalid
                }
            }
            "setconfigjson" => {
                if cmdarg.len() == 2 {
                    WSCommand::SetConfigJson(cmdarg[1].to_string())
                } else {
                    WSCommand::Invalid
                }
            }
            _ => WSCommand::Invalid,
        }
    } else {
        WSCommand::Invalid
    }
}

pub fn start_server(
    port: usize,
    signal_reload: Arc<AtomicBool>,
    signal_exit: Arc<AtomicUsize>,
    active_config_shared: Arc<Mutex<Option<config::Configuration>>>,
    active_config_path: Arc<Mutex<Option<String>>>,
    new_config_shared: Arc<Mutex<Option<config::Configuration>>>,
    capture_status: Arc<RwLock<CaptureStatus>>,
) {
    debug!("Start websocket server on port {}", port);
    thread::spawn(move || {
        ws::listen(format!("127.0.0.1:{}", port), |socket| {
            let signal_reload_inst = signal_reload.clone();
            let signal_exit_inst = signal_exit.clone();
            let active_config_inst = active_config_shared.clone();
            let new_config_inst = new_config_shared.clone();
            let active_config_path_inst = active_config_path.clone();
            let capture_status_inst = capture_status.clone();
            move |msg: ws::Message| {
                let command = parse_command(&msg);
                debug!("parsed command: {:?}", command);
                match command {
                    WSCommand::Reload => {
                        signal_reload_inst.store(true, Ordering::Relaxed);
                        socket.send("OK:RELOAD")
                    }
                    WSCommand::GetCaptureRate => {
                        let capstat = capture_status_inst.read().unwrap();
                        socket.send(format!("OK:GETCAPTURERATE:{}", capstat.measured_samplerate))
                    }
                    WSCommand::GetSignalRange => {
                        let capstat = capture_status_inst.read().unwrap();
                        socket.send(format!("OK:GETSIGNALRANGE:{}", capstat.signal_range))
                    }
                    WSCommand::GetVersion => {
                        socket.send(format!("OK:GETVERSION:{}", crate_version!()))
                    }
                    WSCommand::GetState => {
                        let capstat = capture_status_inst.read().unwrap();
                        socket.send(format!("OK:GETSTATE:{}", &capstat.state.to_string()))
                    }
                    WSCommand::GetRateAdjust => {
                        let capstat = capture_status_inst.read().unwrap();
                        socket.send(format!("OK:GETRATEADJUST:{}", capstat.rate_adjust))
                    }
                    WSCommand::GetUpdateInterval => {
                        let capstat = capture_status_inst.read().unwrap();
                        socket.send(format!("OK:GETUPDATEINTERVAL:{}", capstat.update_interval))
                    }
                    WSCommand::SetUpdateInterval(nbr) => {
                        let mut capstat = capture_status_inst.write().unwrap();
                        capstat.update_interval = nbr;
                        socket.send("OK:SETUPDATEINTERVAL".to_string())
                    }
                    WSCommand::GetConfig => {
                        //let conf_yaml = serde_yaml::to_string(&*active_config_inst.lock().unwrap()).unwrap();
                        socket.send(format!(
                            "OK:GETCONFIG:{}",
                            serde_yaml::to_string(&*active_config_inst.lock().unwrap()).unwrap(),
                        ))
                    }
                    WSCommand::GetConfigJson => {
                        //let conf_yaml = serde_yaml::to_string(&*active_config_inst.lock().unwrap()).unwrap();
                        socket.send(format!(
                            "OK:GETCONFIGJSON:{}",
                            serde_json::to_string(&*active_config_inst.lock().unwrap()).unwrap(),
                        ))
                    }
                    WSCommand::GetConfigName => socket.send(format!(
                        "OK:GETCONFIGNAME:{}",
                        active_config_path_inst
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap_or(&"NONE".to_string())
                            .to_string(),
                    )),
                    WSCommand::SetConfigName(path) => match config::load_validate_config(&path) {
                        Ok(_) => {
                            *active_config_path_inst.lock().unwrap() = Some(path.clone());
                            socket.send(format!("OK:{}", path))
                        }
                        _ => socket.send(format!("ERROR:{}", path)),
                    },
                    WSCommand::SetConfig(config_yml) => {
                        match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                            Ok(conf) => match config::validate_config(conf.clone()) {
                                Ok(()) => {
                                    //*active_config_path_inst.lock().unwrap() = String::from("none");
                                    *new_config_inst.lock().unwrap() = Some(conf);
                                    signal_reload_inst.store(true, Ordering::Relaxed);
                                    socket.send("OK:SETCONFIG")
                                }
                                _ => socket.send("ERROR:SETCONFIG"),
                            },
                            Err(error) => {
                                error!("Config error: {}", error);
                                socket.send("ERROR:SETCONFIG")
                            }
                        }
                    }
                    WSCommand::SetConfigJson(config_json) => {
                        match serde_json::from_str::<config::Configuration>(&config_json) {
                            Ok(conf) => match config::validate_config(conf.clone()) {
                                Ok(()) => {
                                    //*active_config_path_inst.lock().unwrap() = String::from("none");
                                    *new_config_inst.lock().unwrap() = Some(conf);
                                    signal_reload_inst.store(true, Ordering::Relaxed);
                                    socket.send("OK:SETCONFIGJSON")
                                }
                                _ => socket.send("ERROR:SETCONFIGJSON"),
                            },
                            Err(error) => {
                                error!("Config error: {}", error);
                                socket.send("ERROR:SETCONFIGJSON")
                            }
                        }
                    }
                    WSCommand::Stop => {
                        *new_config_inst.lock().unwrap() = None;
                        signal_exit_inst.store(2, Ordering::Relaxed);
                        socket.send("OK:STOP")
                    }
                    WSCommand::Exit => {
                        signal_exit_inst.store(1, Ordering::Relaxed);
                        socket.send("OK:EXIT")
                    }
                    WSCommand::Invalid => {
                        error!("Invalid command {}", msg);
                        socket.send("ERROR:INVALID")
                    }
                }
            }
        })
        .unwrap();
    });
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
