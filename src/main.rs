use std::{collections::HashMap, sync::Arc};

use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

type SequenceIDType = String;

#[derive(Debug, Default)]
pub(crate) struct UserState {}

#[derive(Debug, Default)]
pub(crate) struct WorldState {
    user_states: HashMap<SequenceIDType, UserState>,
}

impl WorldState {}

pub(crate) struct MGNServer {}

impl MGNServer {
    pub(crate) fn new() -> Self {
        Self {}
    }

    pub(crate) async fn run(&self) {
        let world_state: Arc<Mutex<WorldState>> = Arc::new(Mutex::new(WorldState::default()));
        let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();

        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let _world_state = world_state.clone();
            tokio::spawn(async move { MGNServer::process(socket, _world_state).await });
        }
    }

    async fn process(stream: TcpStream, world_state: Arc<Mutex<WorldState>>) {}
}

#[tokio::main]
async fn main() {
    let server = MGNServer::new();
    server.run().await;
}
