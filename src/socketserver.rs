use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, PartialEq)]
enum WSCommand {
    SetConfigName(String),
    Reload,
    GetConfig,
    GetConfigName,
    Exit,
    Invalid,
}

fn parse_command(cmd: ws::Message) -> WSCommand {
    if let Ok(command) = cmd.as_text() {
        let cmdarg: Vec<&str> = command.split(':').collect();
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
                }
                else {
                    WSCommand::Invalid
                }
            },
            _ => WSCommand::Invalid,
        }
    }
    else {
        WSCommand::Invalid
    }
}
//config_path: Arc<Mutex<String>>, 
pub fn start_server(port: usize, signal_reload: Arc<AtomicBool>, active_config: Arc<Mutex<String>>) {
    thread::spawn(move || {
        ws::listen(format!("127.0.0.1:{}", port), |socket| {
            let signal_reload_inst = signal_reload.clone();
            let active_config_inst = active_config.clone();
            move |msg: ws::Message| {
                let command = parse_command(msg);
                match command {
                    WSCommand::Reload => {
                        signal_reload_inst.store(true, Ordering::Relaxed);
                        socket.send("OK")
                    },
                    WSCommand::GetConfig => {
                        socket.send(format!("{}", active_config_inst.lock().unwrap()))
                    },
                    WSCommand::GetConfigName => {
                        socket.send(format!("{}", active_config_inst.lock().unwrap()))
                    },
                    WSCommand::SetConfigName(path) => {
                        socket.send(format!("Change to: {}", path))
                    },
                    WSCommand::Exit => {
                        socket.send("Exiting..")
                    }
                    _ => {
                        Ok(())
                    },
                }
            }
        }).unwrap();
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