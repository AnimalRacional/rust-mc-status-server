use std::{fs, net::{TcpListener, TcpStream}, path::{Path, PathBuf}, str::FromStr, sync::{mpsc, RwLock}, thread};

use lazy_static::lazy_static;
use notify::{event::{AccessKind, AccessMode}, Event, EventKind, RecursiveMode, Result, Watcher};

use crate::{packets::ServerInfo, player::Player};

pub mod packets;
pub mod player;

lazy_static!(
    static ref server_info: RwLock<ServerInfo> = ServerInfo {
        version: String::from("custom"),
        protocol: Some(127),
        online_players: 0,
        max_players: 0,
        player_list: vec![],
        motd: String::from("A status server"),
        icon: None,
        kick_message: String::from("Just a status server")
    }.into();
);

fn handle_client(stream: TcpStream) {
    let mut player = Player::new(stream);
    while match player.receive_packet(&server_info.read().unwrap()) {
        Ok(_) => {
            println!("{}: Finished receiving packet", player.addr);
            true
        },
        Err(e) => {
            println!("Closed connection with {}", player.addr);
            println!("reason {:?}", e);
            false
        }
    } {}
}

fn load_icon(icon_path: &Path) {
    let icon: Option<String> = match fs::read_to_string(icon_path) {
        Ok(icon_b64) => {
            Some(icon_b64)
        },
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
        Ok(text) => {
            match toml::from_str::<ServerInfo>(text) {
                Ok(mut config) => {
                    let mut cfg = server_info.write().unwrap();
                    config.icon = cfg.icon.clone(); // TODO don't clone icon
                    *cfg = config;
                },
                Err(e) => { eprintln!("Couldn't parse config file! {}", e); }
            }
        },
        Err(e) => { eprintln!("Couldn't read config file! {}", e); }
    }
}

fn main() {
    load_config(&PathBuf::from_str("config/config.toml").unwrap());
    load_icon(&PathBuf::from_str("config/icon.b64").unwrap());
    {
        let config = server_info.read().unwrap();
        println!("Loaded config");
        println!("Players: {}/{}", config.online_players, config.max_players);
        for i in &config.player_list {
            println!("- {}", i.name);
        }
        println!("Version {}, Protocol {}", config.version, match config.protocol {
            Some(p) => &p.to_string(),
            None => "same as player"
        });
        println!("Kick message: {}", config.kick_message);
    }
    let (sender, receiver) = mpsc::channel::<Result<Event>>();
    let mut watcher = notify::recommended_watcher(sender).unwrap();
    watcher.watch(Path::new("config"), RecursiveMode::NonRecursive).unwrap();
    println!("Hello, world!");
    let listener = match TcpListener::bind("127.0.0.1:8500") {
        Ok(listener) => listener,
        Err(e) => { eprintln!("Something went wrong while listening for connections! {}", e); return; }
    };
    thread::scope(move |s| {
        s.spawn(move || {
            for client in listener.incoming() {
                match client {
                    Ok(stream) => { std::thread::spawn(move || handle_client(stream)); }
                    Err(e) => { eprintln!("Couldn't get client! {e}"); return; }
                }
            }
        });
        s.spawn(move || {
            for res in receiver {
                match res {
                    Ok(event) => {
                        // TODO there may to be a better way of doing this...
                        if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write)) {
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
                    Err(e) => eprintln!("file watch error {:?}", e)
                }
            }
        });
    })
}
