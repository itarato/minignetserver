extern crate log;
extern crate pretty_env_logger;

use std::{collections::HashMap, sync::Arc};

use log::{error, info, trace};
use minignetcommon::{GamerIdType, Message, MessageAddress, Operation, Response, SessionIdType};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, tcp::WriteHalf},
    sync::Mutex,
};

#[derive(Debug, Default, Clone)]
pub(crate) struct UserUpdate {
    pub update: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct UserState {
    updates: Vec<UserUpdate>,
    awaiting_messages: Vec<Message>,
}

impl UserState {
    pub(crate) fn add_update(&mut self, update: Vec<u8>) {
        self.updates.push(UserUpdate { update });
    }

    pub(crate) fn reset(&mut self) {
        self.updates.clear();
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
    user_states: HashMap<GamerIdType, UserState>,
    sequence: Vec<GamerIdType>,
    current_gamer_index: usize,
    state: GameState,
}

impl GameSession {
    pub(crate) fn new() -> Self {
        Self {
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

        self.user_states
            .insert(gamer_id.clone(), UserState::default());

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

    pub(crate) fn is_game_on(&self) -> bool {
        self.state == GameState::Game
    }

    pub(crate) fn reset(&mut self) {
        self.state = GameState::Join;
        self.current_gamer_index = 0;

        for (_, user_state) in self.user_states.iter_mut() {
            user_state.reset();
        }
    }

    pub(crate) fn start(&mut self) {
        if self.state == GameState::Join {
            self.state = GameState::Game;
            info!("Session has started");
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

    pub(crate) fn add_update(&mut self, gamer_id: GamerIdType, update: Vec<u8>) -> bool {
        match self.user_states.get_mut(&gamer_id) {
            Some(user_state) => {
                user_state.add_update(update);
                true
            }
            None => {
                error!("Gamer is missing missing");
                false
            }
        }
    }

    pub(crate) fn save_message(&mut self, message: Message) {
        match &message.to {
            MessageAddress::All => {
                for gamer_id in &self.sequence {
                    if gamer_id == &message.from {
                        continue;
                    }

                    self.user_states
                        .get_mut(gamer_id)
                        .expect("Missing user state")
                        .awaiting_messages
                        .push(message.clone());
                }
            }
            MessageAddress::One(gamer_id) => {
                self.user_states
                    .get_mut(gamer_id)
                    .expect("Missing user state")
                    .awaiting_messages
                    .push(message.clone());
            }
        }
    }

    pub(crate) fn pop_gamer_messages(&mut self, gamer_id: GamerIdType) -> Vec<Message> {
        self.user_states
            .get_mut(&gamer_id)
            .map(|user_state| {
                let mut out_messages = vec![];
                std::mem::swap(&mut out_messages, &mut user_state.awaiting_messages);
                out_messages
            })
            .unwrap_or(vec![])
    }

    pub(crate) fn next_gamer(&mut self) {
        self.current_gamer_index = (self.current_gamer_index + 1) % self.sequence.len();
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
        let listener = TcpListener::bind("0.0.0.0:8888").await.unwrap();

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
                        trace!("Connection closed");
                        break;
                    }

                    bytes.extend_from_slice(&buf[0..size]);
                    trace!("Received {} bytes", size);
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
                    Operation::ResetSession(session_id) => {
                        MGNServer::process_reset_session(&mut writer, session_id, world_state)
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
                    Operation::IsGameOn(session_id) => {
                        MGNServer::process_is_game_on(&mut writer, session_id, world_state).await;
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
                    Operation::GetPreviousRoundUpdates(session_id) => {
                        MGNServer::process_get_previous_round_updates(
                            &mut writer,
                            session_id,
                            world_state,
                        )
                        .await;
                    }
                    Operation::SendMessage(session_id, message) => {
                        MGNServer::process_send_message(
                            &mut writer,
                            session_id,
                            message,
                            world_state,
                        )
                        .await;
                    }
                    Operation::FetchAllMessages(session_id, gamer_id) => {
                        MGNServer::process_fetch_all_messages(
                            &mut writer,
                            session_id,
                            gamer_id,
                            world_state,
                        )
                        .await;
                    }
                    Operation::NextGamer(session_id) => {
                        MGNServer::process_next_gamer(&mut writer, session_id, world_state).await;
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
        let encoded = bincode::encode_to_vec(&response, bincode::config::standard())
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
                .or_insert(GameSession::new());

            session.join(gamer_id);
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_reset_session(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        {
            let mut state = world_state.lock().await;
            match state.sessions.get_mut(&session_id) {
                Some(session) => session.reset(),
                None => {
                    error!("Session {:?}, it does not exist", session_id);
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            };
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
            match state.sessions.get_mut(&session_id) {
                Some(session) => session.start(),
                None => {
                    error!("Cannot start session {:?}, it does not exist", session_id);
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            };
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
            match state.sessions.get_mut(&session_id) {
                Some(session) => session.end(),
                None => {
                    error!("Cannot start session {:?}, it does not exist", session_id);
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            };
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_is_game_on(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        let is_game_on;
        {
            let mut state = world_state.lock().await;
            is_game_on = match state.sessions.get_mut(&session_id) {
                Some(session) => session.is_game_on(),
                None => {
                    error!("Missing session");
                    MGNServer::reply_client(writer, Response::Error).await;
                    return;
                }
            };
        }

        MGNServer::reply_client(writer, Response::OkWithBool(is_game_on)).await
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

            if !session.add_update(gamer_id, update) {
                error!("Gamer is missing missing");
                MGNServer::reply_client(writer, Response::Error).await;
                return;
            }
        }

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_get_previous_round_updates(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        let mut previous_round_updates = HashMap::new();

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

            for (gamer_id, user_state) in session.user_states.iter() {
                previous_round_updates.insert(
                    gamer_id.clone(),
                    user_state
                        .updates
                        .last()
                        .map(|user_update| user_update.update.clone()),
                );
            }
        }

        MGNServer::reply_client(
            writer,
            Response::OkWithPreviousRoundUpdates(previous_round_updates),
        )
        .await
    }

    async fn process_send_message(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        message: Message,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        let mut state = world_state.lock().await;
        let session = match state.sessions.get_mut(&session_id) {
            Some(session) => session,
            None => {
                error!("Session is missing");
                MGNServer::reply_client(writer, Response::Error).await;
                return;
            }
        };

        session.save_message(message);

        MGNServer::reply_client(writer, Response::Ok).await
    }

    async fn process_fetch_all_messages(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        gamer_id: GamerIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        let mut state = world_state.lock().await;
        let session = match state.sessions.get_mut(&session_id) {
            Some(session) => session,
            None => {
                error!("Session is missing");
                MGNServer::reply_client(writer, Response::Error).await;
                return;
            }
        };

        let messages = session.pop_gamer_messages(gamer_id);
        MGNServer::reply_client(writer, Response::OkWithMessages(messages)).await
    }

    async fn process_next_gamer(
        writer: &mut WriteHalf<'_>,
        session_id: SessionIdType,
        world_state: Arc<Mutex<WorldState>>,
    ) {
        let mut state = world_state.lock().await;
        let session = match state.sessions.get_mut(&session_id) {
            Some(session) => session,
            None => {
                error!("Session is missing");
                MGNServer::reply_client(writer, Response::Error).await;
                return;
            }
        };

        session.next_gamer();

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
