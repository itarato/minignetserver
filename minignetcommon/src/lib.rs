use std::collections::HashMap;

use bincode::{Decode, Encode};

use log::{error, trace};
use tokio::{io::AsyncReadExt, net::tcp::ReadHalf};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub type GamerIdType = String;
pub type SessionIdType = String;

pub async fn read_socket_till_end(reader: &mut ReadHalf<'_>) -> Result<Vec<u8>, Error> {
    let mut buf: [u8; 1024] = [0; 1024];
    let mut bytes = vec![];

    loop {
        match reader.read(&mut buf).await {
            Ok(size) => {
                if size == 0 {
                    trace!("Connection closed");
                    return Ok(bytes);
                }

                bytes.extend_from_slice(&buf[0..size]);
                trace!("Received {} bytes", size);
            }
            Err(err) => {
                error!("Error while reading: {:?}", err);
                return Err(err.into());
            }
        }
    }
}

#[derive(Debug, Decode, Encode, Clone)]
pub enum MessageAddress {
    All,
    One(GamerIdType),
}

#[derive(Debug, Decode, Encode, Clone)]
pub struct Message {
    pub from: GamerIdType,
    pub to: MessageAddress,
    pub payload: Vec<u8>,
}

#[derive(Debug, Decode, Encode)]
pub enum Operation {
    JoinSession(SessionIdType, GamerIdType),
    ResetSession(SessionIdType),
    StartSession(SessionIdType),
    EndSession(SessionIdType),
    IsGamerTurn(SessionIdType, GamerIdType),
    NextGamer(SessionIdType),
    IsGameOn(SessionIdType),
    SendUpdate(SessionIdType, GamerIdType, Vec<u8>),
    GetPreviousRoundUpdates(SessionIdType),
    SendMessage(SessionIdType, Message),
    FetchAllMessages(SessionIdType, GamerIdType),
}

#[derive(Debug, Decode, Encode, Clone)]
pub enum Response {
    Ok,
    Error,
    OkWithBool(bool),
    OkWithPreviousRoundUpdates(HashMap<GamerIdType, Option<Vec<u8>>>),
    OkWithMessages(Vec<Message>),
}
