use clap::Parser;

use std::{
    fs, io,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{PoisonError, RwLock, RwLockReadGuard, mpsc::{self, Receiver}},
    thread,
    time::Duration,
};

use lazy_static::lazy_static;
use notify::{
    Event, EventKind, INotifyWatcher, RecursiveMode, Watcher, event::{AccessKind, AccessMode}
};

use crate::{
    packets::{PacketError, ServerConfig, ServerInfo},
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
    PacketError(PacketError)
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            Self::IOError(e) => write!(f, "{}", e),
            Self::InfoUnlock => write!(f, "Couldn't unlock server info"),
            Self::PacketError(e) => write!(f, "{}", e)
        }
    }
}

impl From<PacketError> for ClientError {
    fn from(value: PacketError) -> Self {
        ClientError::PacketError(value)
    }
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
    println!("Player {} connected!", player.addr);
    let info = &server_info.read()?;
    loop {
        let state = player.receive_packet(&info);
        match state {
            Ok(_) => {
                println!("{}: Finished receiving packet", player.addr);
            }
            Err(e) => {
                println!("Closed connection with {}: {}", player.addr, e);
                if let PacketError::ClosedError = e {
                    return Ok(())
                } else {
                    return Err(ClientError::PacketError(e));
                }
            }
        }
    }
}

fn load_icon(icon_path: &Path) -> io::Result<()>{
    let icon: Option<String> = Some(fs::read_to_string(icon_path)?);
    {
        let mut cfg = server_info.write().unwrap();
        cfg.icon = icon;
    }
    Ok(())
}

#[derive(Debug)]
enum ConfigLoadingError {
    IOError(io::Error),
    ConfigError(toml::de::Error)
}

impl std::fmt::Display for ConfigLoadingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            &ConfigLoadingError::IOError(e) => write!(f, "{}", e),
            &ConfigLoadingError::ConfigError(e) => write!(f, "{}", e)
        }
    }
}

impl From<io::Error> for ConfigLoadingError {
    fn from(value: io::Error) -> Self {
        ConfigLoadingError::IOError(value)
    }
}

impl From<toml::de::Error> for ConfigLoadingError {
    fn from(value: toml::de::Error) -> Self {
        ConfigLoadingError::ConfigError(value)
    }
}

fn load_config(config_path: &Path) -> Result<(), ConfigLoadingError> {
    let text = &fs::read_to_string(config_path)?;
    let new_cfg = toml::from_str::<ServerConfig>(text)?;
    {
        let mut cfg = server_info.write().unwrap();
        cfg.config = new_cfg;
    }
    Ok(())
}

#[derive(Parser)]
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
    println!("Using '{c}' as config dir");
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
    match load_config(&config_path) {
        Ok(_) => { println!("Loaded config {}", config_path.display()); },
        Err(e) => { println!("Error loading config! {}", e); return; }
    }
    match load_icon(&icon_path) {
        Ok(_) => { println!("Loaded icon {}", icon_path.display()); },
        Err(e) => { println!("Error loading icon! {}", e); }
    }
    {
        let info = server_info.read();
        match info {
            Ok(info) => {
                println!("Config has been loaded:");
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
                println!("Motd: '{}'", info.config.motd);
                println!("Kick message: '{}'", info.config.kick_message);
                if let Some(_) = info.icon {
                    println!("Icon was loaded");
                } else {
                    println!("No icon loaded");
                }
            }
            Err(_) => {
                eprintln!("Couldn't unlock server info for reading");
            }
        }
    }
    let (sender, recver) = mpsc::channel::<Result<Event, notify::Error>>();
    let receiver: Option<Receiver<Result<Event, notify::Error>>>;
    let mut watcher: Option<INotifyWatcher> = None;
    match notify::recommended_watcher(sender) {
        Ok(mut wtch) => { 
            match wtch.watch(dbg!(&args.cfgdir.canonicalize().unwrap()), RecursiveMode::Recursive) {
                Err(e) => {
                    eprintln!("Couldn't watch config directory: {}", e);
                    receiver = None;
                },
                Ok(_) => {
                    receiver = Some(recver);
                    watcher = Some(wtch);
                }
            };
        },
        Err(e) => {
            eprintln!("Couldn't start file watcher! {}", e);
            receiver = None;
        }
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
                        let client_thread = thread::Builder::new().name(String::from("Client Handler"));
                        let t = client_thread.spawn_scoped(s, move || handle_client(stream));
                        if let Err(e) = t {
                            eprintln!("Couldn't spawn thread for client! {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("Couldn't get client! {e}");
                        return;
                    }
                }
            }
        });
        if let Some(receiver) = receiver {
            s.spawn(move || {
                println!("Listening for config changes...");
                for res in receiver {
                    match res {
                        Ok(event) => {
                            // TODO there may to be a better way of doing this...
                            if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write))
                            {
                                println!("Detected config directory change change...");
                                for i in event.paths {
                                    if i.ends_with("config.toml") {
                                        match load_config(&i) {
                                            Ok(_) => { println!("Reloaded config"); }
                                            Err(e) => { println!("Couldn't reload icon! {}", e); }
                                        }
                                        break;
                                    } else if i.ends_with("icon.b64") {
                                        match load_icon(&i) {
                                            Ok(_) => { println!("Reloaded icon"); }
                                            Err(e) => { println!("Couldn't reload icon! {}", e); }
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("file watch error {}", e),
                    }
                }
            });
        } else {
            eprintln!("Not listening for config changes!");
        }
    });
    if let Some(mut w) = watcher {
        w.unwatch(&args.cfgdir).ok();
    }
}
