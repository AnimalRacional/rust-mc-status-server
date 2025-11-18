use std::{
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
};

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use crate::packets::{self, PacketError, ServerInfo};

#[derive(Debug)]
pub struct HandshakeInfo {
    pub protocol: u16,
    pub server_addr: String,
    pub server_port: u16,
}

#[derive(Debug)]
pub struct ConnectionStateError(u8);

impl fmt::Display for ConnectionStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid connection state {}", self.0)
    }
}

impl core::error::Error for ConnectionStateError {}

#[repr(u8)]
#[derive(Debug, PartialEq)]
pub enum ConnectionState {
    HANDSHAKING,
    STATUS,
    LOGIN,
    TRANSFER,
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HANDSHAKING => {
                write!(f, "Handshaking")
            }
            Self::STATUS => {
                write!(f, "Status")
            }
            Self::LOGIN => {
                write!(f, "Login")
            }
            Self::TRANSFER => {
                write!(f, "Transfer")
            } //_ => { write!(f, "what") }
        }
    }
}

impl std::convert::TryFrom<u8> for ConnectionState {
    type Error = ConnectionStateError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(ConnectionState::STATUS),
            2 => Ok(ConnectionState::LOGIN),
            3 => Ok(ConnectionState::TRANSFER),
            p => Err(ConnectionStateError(p)),
        }
    }
}

pub struct Player {
    pub connection: TcpStream,
    pub addr: SocketAddr,
    pub state: ConnectionState,
    pub handshake_info: Option<HandshakeInfo>,
}

impl Player {
    pub fn new(connection: TcpStream) -> Self {
        let addr = connection.local_addr().unwrap();
        Player {
            connection,
            addr,
            state: ConnectionState::HANDSHAKING,
            handshake_info: None,
        }
    }

    fn handle_packet<T: Read>(
        &mut self,
        packet: &mut T,
        info: &ServerInfo,
    ) -> Result<(), PacketError> {
        let packet_id = varint::decode_stream(packet)?;
        println!("Packet id {:?} by {}", packet_id, self.addr);
        match packet_id {
            0 => {
                packets::handle_status_login(packet, self, info)?;
            }
            1 => {
                packets::handle_ping(packet, self)?;
            }
            p => {
                eprintln!("Invalid packet {} sent by {}", p, self.addr);
            }
        };
        Ok(())
    }

    fn read_utf16_string(&mut self) -> Result<String, PacketError> {
        let strlen = self.connection.read_u16::<BigEndian>()?;
        if strlen > 255 {
            return Err(PacketError::DataError(strlen.to_be_bytes().to_vec()));
        }
        println!("reading {}", strlen);
        let mut pingstr = Vec::<u16>::new();
        for _ in 0..strlen {
            pingstr.push(self.connection.read_u16::<BigEndian>()?);
        }
        String::from_utf16(&pingstr).or_else(|e| Err(PacketError::FromUtf16Error(e)))
    }

    fn handle_legacy_ping(&mut self, info: &ServerInfo) -> Result<(), PacketError> {
        let packet_identifier = self.connection.read_u8()?;
        if packet_identifier != 0xfa {
            eprintln!(
                "{}: Invalid legacy ping packet identifier {}",
                self.addr, packet_identifier
            );
        }
        let pinghost = self.read_utf16_string()?;
        if !pinghost.eq("MC|PingHost") {
            eprintln!("{}: Unexpected ping string {}", self.addr, pinghost);
        }
        self.connection.read_u16::<BigEndian>()?;
        let protocol = self.connection.read_u8()?;
        let hostname = self.read_utf16_string()?;
        let port = self.connection.read_u32::<BigEndian>()?;
        println!(
            "(legacy) {} connecting to {}:{} protocol version {}",
            self.addr, hostname, port, protocol
        );
        // Send response
        let header = [0x00, 0xa7, 0x00, 0x31, 0x00, 0x00];
        let protocol = match info.config.protocol {
            Some(p) => p,
            None => protocol as u16,
        };
        let response = format!(
            "{}\x00{}\x00{}\x00{}\x00{}\x00",
            protocol,
            info.config.version,
            info.config.motd,
            info.config.online_players,
            info.config.max_players
        );
        self.connection.write_u8(0xff)?;
        self.connection
            .write_u16::<BigEndian>(response.len() as u16)?;
        self.connection.write(&header)?;
        let v: Vec<u16> = response.encode_utf16().collect();
        for v in v {
            self.connection.write_u16::<BigEndian>(v)?;
        }
        Ok(())
    }

    pub fn receive_packet(&mut self, info: &ServerInfo) -> Result<(), PacketError> {
        let packet_size = varint::decode_stream(&mut self.connection).unwrap();
        println!("{} sent packet sized {}", self.addr, packet_size);
        if packet_size <= 0 {
            return Err(PacketError::ClosedError);
        }
        if packet_size > 256 {
            return Err(PacketError::ClosedError);
        }
        if packet_size == 254 && self.state == ConnectionState::HANDSHAKING {
            self.handle_legacy_ping(info)?;
            return Ok(());
        }
        let packet_size = packet_size as usize;
        let mut buf = vec![0; packet_size];
        self.connection.read_exact(&mut buf)?;
        self.handle_packet(&mut buf.as_slice(), info)?;
        Ok(())
    }
}
