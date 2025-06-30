extern crate log;
extern crate pretty_env_logger;

use std::{collections::HashMap, sync::Arc};

use log::{error, info};
use minignetcommon::{GamerIdType, Operation, Response, SessionIdType};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

#[derive(Debug, Default)]
pub(crate) struct UserUpdate {
    update: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct UserState {
    gamer_id: GamerIdType,
    updates: Vec<UserUpdate>,
}

impl UserState {
    pub(crate) fn new(gamer_id: GamerIdType) -> Self {
        Self {
            gamer_id,
            updates: Default::default(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct GameSession {
    session_id: SessionIdType,
    user_states: HashMap<GamerIdType, UserState>,
}

impl GameSession {
    pub(crate) fn new(session_id: SessionIdType) -> Self {
        Self {
            session_id,
            user_states: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct WorldState {
    sessions: HashMap<SessionIdType, GameSession>,
    sequence: Vec<GamerIdType>,
    current: usize,
}

impl WorldState {}

pub(crate) struct MGNServer {}

impl MGNServer {
    pub(crate) fn new() -> Self {
        Self {}
    }

    pub(crate) async fn run(&self) {
        let world_state: Arc<Mutex<WorldState>> = Arc::new(Mutex::new(WorldState::default()));
        let listener = TcpListener::bind("127.0.0.1:8888").await.unwrap();

        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let _world_state = world_state.clone();
            tokio::spawn(async move { MGNServer::process(socket, _world_state).await });
        }
    }

    async fn process(mut stream: TcpStream, world_state: Arc<Mutex<WorldState>>) {
        let mut bytes: Vec<u8> = vec![];
        let (mut reader, mut writer) = stream.split();
        let mut buf: [u8; 1024] = [0; 1024];

        loop {
            match reader.read(&mut buf).await {
                Ok(size) => {
                    if size == 0 {
                        info!("Connection closed");
                        break;
                    }

                    bytes.extend_from_slice(&buf[0..size]);
                    info!("Received {} bytes", size);
                }
                Err(err) => {
                    error!("Error while reading: {:?}", err);
                    return;
                }
            }
        }

        let op: Result<(Operation, usize), bincode::error::DecodeError> =
            bincode::decode_from_slice(&bytes[..], bincode::config::standard());

        match op {
            Ok((Operation::JoinSession(session_id, gamer_id), _len)) => {
                info!(
                    "Received JOIN-SESSION message for session id: {:?} and gamer id: {:?}",
                    session_id, gamer_id
                );

                world_state
                    .lock()
                    .await
                    .sessions
                    .entry(session_id.clone())
                    .or_insert(GameSession::new(session_id))
                    .user_states
                    .insert(gamer_id.clone(), UserState::new(gamer_id));

                let ok_encoded = bincode::encode_to_vec(Response::Ok, bincode::config::standard())
                    .expect("Failed encoding ok message");
                if let Err(err) = writer.write(&ok_encoded[..]).await {
                    error!("Failed responsing ok: {:?}", err);
                }
            }
            Err(err) => {
                error!("Failed decoding input: {:?}", err);

                let err_encoded =
                    bincode::encode_to_vec(Response::Error, bincode::config::standard())
                        .expect("Failed encoding error");

                if let Err(err) = writer.write(&err_encoded[..]).await {
                    error!("Failed responding error: {:?}", err);
                }
            }
        }

        if let Err(err) = writer.shutdown().await {
            error!("Failed to shut down writer: {:?}", err);
        }
    }
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("Server has started");

    let server = MGNServer::new();
    server.run().await;
}
