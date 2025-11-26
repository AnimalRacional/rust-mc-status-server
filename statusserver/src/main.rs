use clap::Parser;

use std::{
    fs, io,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{mpsc, PoisonError, RwLock, RwLockReadGuard},
    thread,
    time::Duration,
};

use lazy_static::lazy_static;
use notify::{
    event::{AccessKind, AccessMode},
    Event, EventKind, RecursiveMode, Watcher,
};

use crate::{
    packets::{ServerConfig, ServerInfo},
    player::Player,
};

pub mod packets;
pub mod player;

lazy_static! {
    static ref server_info: RwLock<ServerInfo> = ServerInfo {
        config: ServerConfig {
            version: String::from("custom"),
            protocol: Some(127),
            online_players: 0,
            max_players: 0,
            player_list: vec![],
            motd: String::from("A status server"),
            kick_message: String::from("Just a status server")
        },
        icon: None,
    }
    .into();
}

enum ClientError {
    IOError(io::Error),
    InfoUnlock,
}

impl From<io::Error> for ClientError {
    fn from(value: io::Error) -> Self {
        ClientError::IOError(value)
    }
}

impl From<PoisonError<RwLockReadGuard<'_, ServerInfo>>> for ClientError {
    fn from(_: PoisonError<RwLockReadGuard<'_, ServerInfo>>) -> Self {
        ClientError::InfoUnlock
    }
}

fn handle_client(stream: TcpStream) -> Result<(), ClientError> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut player = Player::new(stream);
    let info = &server_info.read()?;
    loop {
        let state = player.receive_packet(&info);
        match state {
            Ok(_) => {
                println!("{}: Finished receiving packet", player.addr);
            }
            Err(e) => {
                println!("Closed connection with {}", player.addr);
                println!("reason: {:?}", e);
                break;
            }
        }
    }
    Ok(())
}

fn load_icon(icon_path: &Path) {
    let icon: Option<String> = match fs::read_to_string(icon_path) {
        Ok(icon_b64) => Some(icon_b64),
        Err(e) => {
            eprintln!("Couldn't read icon from icon.b64 file! {}", e);
            None
        }
    };
    {
        let mut cfg = server_info.write().unwrap();
        cfg.icon = icon;
    }
}

fn load_config(config_path: &Path) {
    match &fs::read_to_string(config_path) {
        Ok(text) => match toml::from_str::<ServerConfig>(text) {
            Ok(config) => {
                let mut cfg = server_info.write().unwrap();
                cfg.config = config;
            }
            Err(e) => {
                eprintln!("Couldn't parse config file! {}", e);
            }
        },
        Err(e) => {
            eprintln!("Couldn't read config file! {}", e);
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about)]
struct CommandArgs {
    // The host to open on
    #[arg(short, long, default_value_t = String::from("127.0.0.1:25565"))]
    ip: String,
    #[arg(short, long, default_value = "./config")]
    cfgdir: PathBuf,
}

fn main() {
    let args = CommandArgs::parse();
    let c = args.cfgdir.display();
    println!("Using {c} as config dir");
    let config_path = {
        let mut c = args.cfgdir.clone();
        c.push("config.toml");
        c
    };
    let icon_path = {
        let mut c = args.cfgdir.clone();
        c.push("icon.b64");
        c
    };
    load_config(&config_path);
    load_icon(&icon_path);
    {
        let info = server_info.read();
        match info {
            Ok(info) => {
                println!("Loaded config");
                println!(
                    "Players: {}/{}",
                    info.config.online_players, info.config.max_players
                );
                for i in &info.config.player_list {
                    println!("- {}", i.name);
                }
                println!(
                    "Version {}, Protocol {}",
                    info.config.version,
                    match info.config.protocol {
                        Some(p) => &p.to_string(),
                        None => "same as player",
                    }
                );
                println!("Kick message: {}", info.config.kick_message);
            }
            Err(_) => {
                eprintln!("Couldn't unlock server info for reading");
            }
        }
    }
    let receiver = 'receiverval: {
        let (sender, recver) = mpsc::channel::<Result<Event, notify::Error>>();
        let mut watcher = match notify::recommended_watcher(sender) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Couldn't start file watcher! {}", e);

                break 'receiverval None;
            }
        };
        let w = watcher.watch(&args.cfgdir, RecursiveMode::NonRecursive);
        if let Err(e) = w {
            eprintln!("Couldn't watch config directory: {}", e);
            break 'receiverval None;
        }

        Some(recver)
    };

    println!("Hello, world!");
    let listener = match TcpListener::bind(&args.ip) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!(
                "Something went wrong while listening for connections! {}",
                e
            );
            return;
        }
    };
    println!("Listening on {}", args.ip);
    thread::scope(move |s| {
        s.spawn(move || {
            for client in listener.incoming() {
                match client {
                    Ok(stream) => {
                        std::thread::spawn(move || handle_client(stream));
                    }
                    Err(e) => {
                        eprintln!("Couldn't get client! {e}");
                        return;
                    }
                }
            }
        });
        if let Some(receiver) = receiver {
            println!("Listening for config changes...");
            s.spawn(move || {
                for res in receiver {
                    match res {
                        Ok(event) => {
                            // TODO there may to be a better way of doing this...
                            if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write))
                            {
                                for i in event.paths {
                                    if i.ends_with("config.toml") {
                                        load_config(&i);
                                        println!("Reloaded config");
                                        break;
                                    } else if i.ends_with("icon.b64") {
                                        load_icon(&i);
                                        println!("Reloaded icon");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("file watch error {:?}", e),
                    }
                }
            });
        } else {
            eprintln!("Not listening for config changes!");
        }
    })
}
