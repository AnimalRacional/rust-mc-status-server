use byteorder::{BigEndian, ReadBytesExt};
use json::{object, JsonValue};
use log::{debug, error, info};
use serde::Deserialize;
use std::{
    io::{Error, Read, Write},
    str::Utf8Error,
    string::{FromUtf16Error, FromUtf8Error},
};
use uuid::Uuid;

use crate::player::{ConnectionState, HandshakeInfo, Player};

const DEFAULT_UUID: Uuid = *uuid::Builder::from_bytes([0u8; 16]).as_uuid();

#[derive(Debug)]
pub enum PacketError {
    IOError(Error),
    FromUtf8Error(FromUtf8Error),
    Utf8Error(Utf8Error),
    FromUtf16Error(FromUtf16Error),
    DataError(Vec<u8>),
    ClosedError,
}

impl std::fmt::Display for PacketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IOError(e) => write!(f, "{}", e) ,
            Self::FromUtf8Error(e) => write!(f, "Invalid string sent: {}", e),
            Self::Utf8Error(e) => write!(f, "Invalid string sent: {}", e),
            Self::FromUtf16Error(e) => write!(f, "Invalid legacy string sent: {}", e),
            Self::DataError(e) => write!(f, "Player sent invalid data: {:?}", e),
            Self::ClosedError => write!(f, "Connection closed")
        }
    }
}

impl From<std::io::Error> for PacketError {
    fn from(value: std::io::Error) -> Self {
        PacketError::IOError(value)
    }
}

impl From<FromUtf8Error> for PacketError {
    fn from(value: FromUtf8Error) -> Self {
        PacketError::FromUtf8Error(value)
    }
}

impl From<Utf8Error> for PacketError {
    fn from(value: Utf8Error) -> Self {
        PacketError::Utf8Error(value)
    }
}

impl From<FromUtf16Error> for PacketError {
    fn from(value: FromUtf16Error) -> Self {
        PacketError::FromUtf16Error(value)
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct PlayerListEntry {
    pub name: String,
    pub uuid: Option<Uuid>,
}

impl From<(&str, Option<Uuid>)> for PlayerListEntry {
    fn from(value: (&str, Option<Uuid>)) -> Self {
        PlayerListEntry {
            name: value.0.to_string(),
            uuid: value.1,
        }
    }
}

impl From<PlayerListEntry> for JsonValue {
    fn from(value: PlayerListEntry) -> Self {
        object! {
            name: value.name.as_str(),
            id: value.uuid.unwrap_or(DEFAULT_UUID).to_string()
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct ServerConfig {
    pub version: String,
    pub protocol: Option<u16>,
    pub online_players: i32,
    pub max_players: i32,
    pub player_list: Vec<PlayerListEntry>,
    pub motd: String,
    pub kick_message: String,
}

#[derive(Deserialize, Debug)]
pub struct ServerInfo {
    pub config: ServerConfig,
    pub icon: Option<String>,
}

pub fn handle_status_login<T: Read>(
    packet: &mut T,
    client: &mut Player,
    info: &ServerInfo,
) -> Result<(), PacketError> {
    debug!("Received status/login packet from {}", client.addr);
    let state = &client.state;
    match state {
        ConnectionState::HANDSHAKING => {
            handle_handshake(packet, client)?;
        }
        ConnectionState::STATUS => {
            handle_status(packet, client, info)?;
        }
        ConnectionState::LOGIN => {
            handle_login(packet, client, info)?;
        }
        s => {
            error!(
                "Invalid request: packet 0 in state {} by {}",
                s, client.addr
            );
        }
    };
    Ok(())
}

fn handle_handshake<T: Read>(packet: &mut T, client: &mut Player) -> Result<(), PacketError> {
    debug!("Received handshake packet from {}", client.addr);
    let stream = packet;
    let protocol_version = varint::decode_stream(stream)? as u16;
    let strlen = varint::decode_stream(stream)? as usize;
    let mut strbuf = vec![0u8; strlen];
    stream.read_exact(&mut strbuf)?;
    let host = String::from_utf8(strbuf)?;
    let port = stream.read_u16::<BigEndian>()?;
    let intent = varint::decode_stream(stream)?;
    let intent = ConnectionState::try_from(intent as u8)
        .or_else(|_| Err(PacketError::DataError(vec![intent as u8])))?;
    info!(
        "{}:{} connected with protocol {} intent {}",
        host, port, protocol_version, intent
    );
    let info = HandshakeInfo {
        protocol: protocol_version,
        server_addr: host,
        server_port: port,
    };
    client.handshake_info = Some(info);
    client.state = intent;
    Ok(())
}

fn make_status_response(
    version: &str,
    protocol: u16,
    maxplr: i32,
    players: i32,
    playerlist: &Vec<PlayerListEntry>,
    motd: &str,
    secure: bool,
    icon: Option<&str>,
) -> String {
    let motd = json::parse(motd).unwrap_or(JsonValue::String(motd.to_string()));
    let icon = match icon {
        Some(i) => Some(format!("data:image/png;base64,{i}")),
        None => None,
    };
    let obj = object! {
        version: {
            name: version,
            protocol: protocol
        },
        players: {
            max: maxplr,
            online: players,
            sample: playerlist.clone() // FIXME way of doing this without clone?
        },
        description: motd,
        favicon: icon,
        enforcesSecureChat: secure,
    };
    obj.to_string()
}

fn send_packet(packet_id: i32, data: &[u8], client: &mut Player) -> Result<(), PacketError> {
    let mut packet_id = varint::encode(packet_id);
    let mut total_packet = varint::encode((packet_id.len() + data.len()) as i32);
    total_packet.append(&mut packet_id);
    total_packet.append(&mut data.to_vec());
    if let Err(e) = client.connection.write(total_packet.as_slice()) {
        return Err(PacketError::IOError(e));
    }
    Ok(())
}

pub fn handle_ping<T: Read>(data: &mut T, client: &mut Player) -> Result<(), PacketError> {
    debug!("{}: Ping packet", client.addr);
    let pong = data.read_u64::<BigEndian>()?;
    send_packet(0x01, &pong.to_be_bytes(), client)?;
    Ok(())
}

fn handle_status<T: Read>(
    _: &mut T,
    client: &mut Player,
    info: &ServerInfo,
) -> Result<(), PacketError> {
    debug!("Received status packet from {}", client.addr);
    let protocol: u16 = match info.config.protocol {
        Some(p) => p,
        None => match &client.handshake_info {
            Some(p) => p.protocol,
            None => 127,
        },
    };
    let response = make_status_response(
        &info.config.version,
        protocol,
        info.config.max_players,
        info.config.online_players,
        &info.config.player_list,
        &info.config.motd,
        false,
        info.icon.as_deref(),
    );
    let response = response.as_bytes();
    let mut full_data = varint::encode(response.len() as i32);
    full_data.extend(response);
    send_packet(0x00, full_data.as_slice(), client)?;
    Ok(())
}

fn handle_login<T: Read>(
    packet: &mut T,
    client: &mut Player,
    info: &ServerInfo,
) -> Result<(), PacketError> {
    debug!("Received login packet from {}", client.addr);
    let name_len = varint::decode_stream(packet)?;
    if name_len <= 0 || name_len > 16 {
        error!("Invalid name length {}", name_len);
        client.connection.shutdown(std::net::Shutdown::Both)?;
        return Err(PacketError::DataError(name_len.to_be_bytes().to_vec()));
    }
    let mut namebuf = [0u8; 16];
    let (namebuf, _) = namebuf.split_at_mut(name_len as usize);
    packet.read_exact(namebuf)?;
    let name = str::from_utf8(namebuf)?;
    let uuid = packet.read_u128::<BigEndian>()?;
    info!("Player login: {} {}", name, uuid);
    let kick_message = match json::parse(&info.config.kick_message) {
        Ok(v) => v.to_string(),
        Err(_) => info.config.kick_message.to_string()
    };
    let mut total_data = varint::encode(kick_message.len() as i32);
    total_data.extend(kick_message.as_bytes());
    send_packet(0x00, total_data.as_slice(), client)?;
    Ok(())
}
