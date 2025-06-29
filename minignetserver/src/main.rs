extern crate log;
extern crate pretty_env_logger;

use std::{collections::HashMap, sync::Arc};

use log::{error, info};
use minignetcommon::{Operation, Response, SequenceIDType};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

#[derive(Debug, Default)]
pub(crate) struct UserUpdate {
    update: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct UserState {
    updates: Vec<UserUpdate>,
}

impl UserState {}

#[derive(Debug, Default)]
pub(crate) struct WorldState {
    user_states: HashMap<SequenceIDType, UserState>,
    sequence: Vec<SequenceIDType>,
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
            Ok((Operation::JoinSession(id), _len)) => {
                info!("Received JOIN-SESSION message for id: {:?}", id);

                world_state
                    .lock()
                    .await
                    .user_states
                    .insert(id, UserState::default());

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
