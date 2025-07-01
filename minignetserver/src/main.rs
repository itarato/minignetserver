extern crate log;
extern crate pretty_env_logger;

use std::{collections::HashMap, sync::Arc};

use log::{error, info};
use minignetcommon::{GamerIdType, Operation, Response, SessionIdType};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, tcp::WriteHalf},
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

    pub(crate) fn add_update(&mut self, update: Vec<u8>) {
        self.updates.push(UserUpdate { update });
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum GameState {
    Join,
    Game,
    Over,
}

#[derive(Debug)]
pub(crate) struct GameSession {
    session_id: SessionIdType,
    user_states: HashMap<GamerIdType, UserState>,
    sequence: Vec<GamerIdType>,
    current_gamer_index: usize,
    state: GameState,
}

impl GameSession {
    pub(crate) fn new(session_id: SessionIdType) -> Self {
        Self {
            session_id,
            user_states: HashMap::new(),
            current_gamer_index: 0,
            state: GameState::Join,
            sequence: vec![],
        }
    }

    pub(crate) fn join(&mut self, gamer_id: GamerIdType) {
        if self.user_states.contains_key(&gamer_id) {
            // When it already exists - consider signalling so the client can fetch the
            // previous state (aka re-join).
            return;
        }

        self.user_states.insert(
            gamer_id.clone(),
            UserState {
                gamer_id: gamer_id.clone(),
                updates: Default::default(),
            },
        );

        self.sequence.push(gamer_id);
    }

    pub(crate) fn is_gamer_turn(&self, gamer_id: GamerIdType) -> bool {
        if self.state != GameState::Game {
            return false;
        }

        self.sequence
            .iter()
            .position(|id| id == &gamer_id)
            .map(|pos| pos == self.current_gamer_index)
            .unwrap_or(false)
    }

    pub(crate) fn start(&mut self) {
        if self.state == GameState::Join {
            self.state = GameState::Game;
        } else {
            error!("Starting a session that is not in JOIN state");
        }
    }

    pub(crate) fn end(&mut self) {
        if self.state == GameState::Game {
            self.state = GameState::Over;
        } else {
            error!("Ending a session that is not in GAME state");
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct WorldState {
    sessions: HashMap<SessionIdType, GameSession>,
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
            Ok((operation, ..)) => {
                info!("Received operation: {:?}", &operation);

                match operation {
                    Operation::JoinSession(session_id, gamer_id) => {
                        MGNServer::process_join_session(
                            &mut writer,
                            session_id,
                            gamer_id,
                            world_state,
                        )
                        .await;
                    }
                    Operation::StartSession(session_id) => {
                        MGNServer::process_start_session(&mut writer, session_id, world_state)
                            .await;
                    }
                    Operation::EndSession(session_id) => {
                        MGNServer::process_end_session(&mut writer, session_id, world_state).await;
                    }
                    Operation::IsGamerTurn(session_id, gamer_id) => {
                        MGNServer::process_is_gamer_turn(
                            &mut writer,
                            session_id,
                            gamer_id,
                            world_state,
                        )
                        .await;
                    }
                    Operation::SendUpdate(session_id, gamer_id, update) => {
                        MGNServer::process_send_update(
                            &mut writer,
                            session_id,
                            gamer_id,
                            update,
                            world_state,
                        )
                        .await;
                    }
                };
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

    async fn reply_client(writer: &mut WriteHalf<'_>, response: Response) {
        let encoded = bincode::encode_to_vec(response.clone(), bincode::config::standard())
            .expect(&format!("Failed encoding response message: {:?}", response));
        if let Err(err) = writer.write(&encoded[..]).await {
            error!("Failed responding to client: {:?}", err);
        }
    }

    async fn process_join_session(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        gamer_id: GamerIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        {
            let mut state = world_state.lock().await;
            let session = state
                .sessions
                .entry(session_id.clone())
                .or_insert(GameSession::new(session_id));

            session.join(gamer_id);
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_start_session(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        {
            let mut state = world_state.lock().await;
            state
                .sessions
                .get_mut(&session_id)
                .map(|session| session.start())
                .unwrap_or_else(|| {
                    error!("Cannot start session {:?}, it does not exist", session_id);
                });
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_end_session(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        {
            let mut state = world_state.lock().await;
            state
                .sessions
                .get_mut(&session_id)
                .map(|session| session.end())
                .unwrap_or_else(|| {
                    error!("Cannot start session {:?}, it does not exist", session_id);
                });
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_is_gamer_turn(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        gamer_id: GamerIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        let is_gamer_turn;
        {
            let mut state = world_state.lock().await;
            is_gamer_turn = match state.sessions.get_mut(&session_id) {
                Some(session) => session.is_gamer_turn(gamer_id),
                None => {
                    error!("Missing session");
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            };
        }

        MGNServer::reply_client(writer, Response::OkWithBool(is_gamer_turn)).await
    }

    async fn process_send_update(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        gamer_id: GamerIdType,
        update: Vec<u8>,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        {
            let mut state = world_state.lock().await;
            let session = match state.sessions.get_mut(&session_id) {
                Some(session) => session,
                None => {
                    error!("Session is missing");
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            };
            match session.user_states.get_mut(&gamer_id) {
                Some(user_state) => user_state.add_update(update),
                None => {
                    error!("Gamer is missing missing");
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            }
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("Server has started");

    let server = MGNServer::new();
    server.run().await;
}
