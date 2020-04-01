use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use config;

#[derive(Debug, PartialEq)]
enum WSCommand {
    SetConfigName(String),
    SetConfig(String),
    Reload,
    GetConfig,
    GetConfigName,
    Exit,
    Invalid,
}

fn parse_command(cmd: ws::Message) -> WSCommand {
    if let Ok(command) = cmd.as_text() {
        let cmdarg: Vec<&str> = command.splitn(2, ':').collect();
        if cmdarg.is_empty() {
            return WSCommand::Invalid;
        }
        match cmdarg[0] {
            "reload" => WSCommand::Reload,
            "getconfig" => WSCommand::GetConfig,
            "getconfigname" => WSCommand::GetConfigName,
            "exit" => WSCommand::Exit,
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
            _ => WSCommand::Invalid,
        }
    } else {
        WSCommand::Invalid
    }
}

pub fn start_server(
    port: usize,
    signal_reload: Arc<AtomicBool>,
    signal_exit: Arc<AtomicBool>,
    active_config_shared: Arc<Mutex<config::Configuration>>,
    active_config_path: Arc<Mutex<String>>,
) {
    debug!("Start websocket server on port {}", port);
    thread::spawn(move || {
        ws::listen(format!("127.0.0.1:{}", port), |socket| {
            let signal_reload_inst = signal_reload.clone();
            let signal_exit_inst = signal_exit.clone();
            let active_config_inst = active_config_shared.clone();
            let active_config_path_inst = active_config_path.clone();
            move |msg: ws::Message| {
                let command = parse_command(msg);
                match command {
                    WSCommand::Reload => {
                        signal_reload_inst.store(true, Ordering::Relaxed);
                        socket.send("OK:RELOAD")
                    }
                    WSCommand::GetConfig => {
                        //let conf_yaml = serde_yaml::to_string(&*active_config_inst.lock().unwrap()).unwrap();
                        socket.send(
                            serde_yaml::to_string(&*active_config_inst.lock().unwrap()).unwrap(),
                        )
                    }
                    WSCommand::GetConfigName => {
                        socket.send(format!("{}", active_config_path_inst.lock().unwrap()))
                    }
                    WSCommand::SetConfigName(path) => match config::load_validate_config(&path) {
                        Ok(_) => {
                            *active_config_path_inst.lock().unwrap() = path.clone();
                            socket.send(format!("OK:{}", path))
                        }
                        _ => socket.send(format!("ERROR:{}", path)),
                    },
                    WSCommand::SetConfig(config_yml) => {
                        match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                            Ok(conf) => match config::validate_config(conf.clone()) {
                                Ok(()) => {
                                    //*active_config_path_inst.lock().unwrap() = String::from("none");
                                    *active_config_inst.lock().unwrap() = conf;
                                    signal_reload_inst.store(true, Ordering::Relaxed);
                                    socket.send("OK:SETCONFIG")
                                }
                                _ => socket.send("ERROR:SETCONFIG"),
                            },
                            _ => socket.send("ERROR:SETCONFIG"),
                        }
                    }
                    WSCommand::Exit => {
                        signal_exit_inst.store(true, Ordering::Relaxed);
                        socket.send("OK:EXIT")
                    }
                    WSCommand::Invalid => socket.send("ERROR:INVALID"),
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
        let res = parse_command(cmd);
        assert_eq!(res, WSCommand::Reload);
        let cmd = Message::text("asdfasdf");
        let res = parse_command(cmd);
        assert_eq!(res, WSCommand::Invalid);
        let cmd = Message::text("");
        let res = parse_command(cmd);
        assert_eq!(res, WSCommand::Invalid);
        let cmd = Message::text("setconfigname:somefile");
        let res = parse_command(cmd);
        assert_eq!(res, WSCommand::SetConfigName("somefile".to_string()));
    }
}
