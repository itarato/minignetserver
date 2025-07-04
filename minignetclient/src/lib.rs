extern crate log;

use std::net::{SocketAddr, ToSocketAddrs};

use log::error;
use minignetcommon::{
    Error, GamerIdType, Message, Operation, Response, SessionIdType, read_socket_till_end,
};
use tokio::{io::AsyncWriteExt, net::TcpStream};

#[derive(Clone)]
pub struct MGNClient {
    serialization_config: bincode::config::Configuration,
    addr: SocketAddr,
    pub session_id: SessionIdType,
    pub gamer_id: GamerIdType,
}

impl MGNClient {
    pub fn new<Addr>(
        addr: Addr,
        session_id: SessionIdType,
        gamer_id: GamerIdType,
    ) -> Result<Self, std::io::Error>
    where
        Addr: ToSocketAddrs,
    {
        let mut address_options = addr.to_socket_addrs().expect("msg");
        let first_address = address_options.next().ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No socket addresses found",
        ))?;

        Ok(Self {
            serialization_config: bincode::config::standard(),
            addr: first_address,
            session_id,
            gamer_id,
        })
    }

    async fn send_message_to_server(&self, op: Operation) -> Result<Response, Error> {
        let op_encoded = bincode::encode_to_vec(op, self.serialization_config)?;

        match TcpStream::connect(self.addr).await {
            Ok(mut stream) => {
                let (mut reader, mut writer) = stream.split();
                if let Err(err) = writer.write_all(&op_encoded[..]).await {
                    error!("Failed writing request: {:?}", err);
                    return Err(err.into());
                }
                writer
                    .shutdown()
                    .await
                    .expect("Failed shutting down writer");

                let response_bytes = read_socket_till_end(&mut reader).await?;
                let (decoded, _size): (Response, usize) =
                    bincode::decode_from_slice(&response_bytes[..], self.serialization_config)?;

                return Ok(decoded);
            }
            Err(e) => {
                eprintln!("Failed to connect: {}", e);
                return Err(e.into());
            }
        }
    }

    pub async fn join_session(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::JoinSession(
            self.session_id.clone(),
            self.gamer_id.clone(),
        ))
        .await
    }

    pub async fn reset_session(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::ResetSession(self.session_id.clone()))
            .await
    }

    pub async fn start_session(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::StartSession(self.session_id.clone()))
            .await
    }

    pub async fn end_session(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::EndSession(self.session_id.clone()))
            .await
    }

    pub async fn is_gamer_turn(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::IsGamerTurn(
            self.session_id.clone(),
            self.gamer_id.clone(),
        ))
        .await
    }

    pub async fn is_game_on(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::IsGameOn(self.session_id.clone()))
            .await
    }

    pub async fn send_update(&self, update: Vec<u8>) -> Result<Response, Error> {
        self.send_message_to_server(Operation::SendUpdate(
            self.session_id.clone(),
            self.gamer_id.clone(),
            update,
        ))
        .await
    }

    pub async fn get_previous_round_updates(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::GetPreviousRoundUpdates(self.session_id.clone()))
            .await
    }

    pub async fn send_message(&self, message: Message) -> Result<Response, Error> {
        self.send_message_to_server(Operation::SendMessage(self.session_id.clone(), message))
            .await
    }

    pub async fn fetch_all_messages(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::FetchAllMessages(
            self.session_id.clone(),
            self.gamer_id.clone(),
        ))
        .await
    }

    pub async fn next_gamer(&self) -> Result<Response, Error> {
        self.send_message_to_server(Operation::NextGamer(self.session_id.clone()))
            .await
    }
}
