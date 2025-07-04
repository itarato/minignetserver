extern crate log;
extern crate pretty_env_logger;

use bincode::{Decode, Encode};
use log::{error, info, warn};
use std::{collections::VecDeque, sync::Arc, time::Duration};

use clap::Parser;
use minignetclient::MGNClient;
use minignetcommon::{Error, GamerIdType, Message, MessageAddress, Response, SessionIdType};
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

#[derive(Parser, Debug)]
struct CmdLineArgs {
    #[arg(short, long)]
    gamer_id: String,

    #[arg(long)]
    session_id: SessionIdType,

    #[arg(long)]
    server: GamerIdType,
}

#[derive(Debug, Clone, Copy)]
enum CellState {
    Undiscovered,
    Hit,
    Miss,
}

enum GameState {
    Initialize,
    ReadyToGuess,
    OtherTurn,
    WaitForReply,
}

#[derive(Debug, Decode, Encode)]
struct Coord {
    x: u8,
    y: u8,
}

#[derive(Debug, Decode, Encode)]
enum TorpedoMessage {
    Guess(Coord),
    HitOrMissReply(Coord, bool),
}

enum InputCommand {
    Start,
    Step(Coord),
}

struct InputParser;

impl InputParser {
    fn parse(raw: String) -> Result<InputCommand, Error> {
        if raw == "start" {
            return Ok(InputCommand::Start);
        }

        if let Some(caps) = regex::Regex::new(r"^([a-j]) (\d{1,2})$")
            .unwrap()
            .captures(&raw)
        {
            let col = caps.get(1).unwrap().as_str().chars().next().unwrap() as u8 - b'a';
            let row: u8 = caps.get(2).unwrap().as_str().parse().unwrap();
            return Ok(InputCommand::Step(Coord { x: col, y: row }));
        }

        Err(format!("Unparsable command: {:?}", raw).into())
    }
}

struct Game {
    self_board: [CellState; 100],
    other_board: [CellState; 100],
    state: GameState,
    client: MGNClient,
    event_queue: Arc<Mutex<VecDeque<Event>>>,
    cmd_queue: Arc<Mutex<VecDeque<Command>>>,
}

impl Game {
    fn new(
        client: MGNClient,
        event_queue: Arc<Mutex<VecDeque<Event>>>,
        cmd_queue: Arc<Mutex<VecDeque<Command>>>,
    ) -> Self {
        Self {
            self_board: [CellState::Undiscovered; 100],
            other_board: [CellState::Undiscovered; 100],
            state: GameState::Initialize,
            client,
            event_queue,
            cmd_queue,
        }
    }

    async fn init(&self) {
        match self.client.join_session().await {
            Ok(Response::Ok) => info!("Joined session"),
            response => panic!("Unexpected response for join: {:?}", response),
        }

        self.cmd_queue
            .lock()
            .await
            .push_front(Command::WaitForTurn(true));
    }

    async fn run(&mut self) {
        loop {
            self.execute_input_command_if_any().await;
            self.consume_events().await;

            match self.state {
                GameState::Initialize => match self.client.is_game_on().await {
                    Ok(Response::OkWithBool(is_game_on)) => {
                        if is_game_on {
                            self.state = GameState::OtherTurn;
                        }
                    }
                    response => {
                        panic!("Unexpected response for IS_GAME_ON: {:?}", response);
                    }
                },
                GameState::ReadyToGuess => {}
                GameState::WaitForReply => {}
                GameState::OtherTurn => match self.client.is_gamer_turn().await {
                    Ok(Response::OkWithBool(is_gamer_turn)) => {
                        if is_gamer_turn {
                            self.state = GameState::ReadyToGuess;
                        }
                    }
                    response => {
                        panic!("Unexpected response for IS_GAMER_TURN: {:?}", response)
                    }
                },
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn read_stdin_line() -> Option<String> {
        let mut stdin = BufReader::new(io::stdin()).lines();

        match stdin.next_line().await {
            Ok(line) => line,
            _ => None,
        }
    }

    async fn execute_input_command_if_any(&mut self) {
        info!("CHECK INPUT");
        if let Some(stdin_line) = Game::read_stdin_line().await {
            info!("Got raw input: {}", stdin_line);

            match InputParser::parse(stdin_line) {
                Ok(cmd) => match cmd {
                    InputCommand::Start => self.cmd_queue.lock().await.push_front(Command::Start),
                    InputCommand::Step(coord_guess) => {
                        match self
                            .client
                            .send_message(Message {
                                from: self.client.gamer_id.clone(),
                                to: MessageAddress::All,
                                payload: bincode::encode_to_vec(
                                    TorpedoMessage::Guess(coord_guess),
                                    bincode::config::standard(),
                                )
                                .expect("Failed encoding guess"),
                            })
                            .await
                        {
                            Ok(Response::Ok) => {
                                self.state = GameState::WaitForReply;
                            }
                            response => panic!("Unexpected respone to update send: {:?}", response),
                        }
                    }
                },
                Err(err) => {
                    error!("Cannot parse command: {:?}", err);
                }
            }
        }
    }

    async fn consume_events(&mut self) {
        info!("CHECK EVENTS");
        let mut _event_queue = self.event_queue.lock().await;
        while let Some(event) = _event_queue.pop_back() {
            info!("Got event: {:?}", &event);

            match event {
                Event::TurnIsActive(is_self) => {
                    if is_self {
                        self.state = GameState::ReadyToGuess;
                    } else {
                        self.state = GameState::OtherTurn;
                    }
                }
                Event::GotGuess(coord) => {
                    let is_hit = coord.x == 1 && coord.y == 1;
                    match self
                        .client
                        .send_message(Message {
                            from: self.client.gamer_id.clone(),
                            to: MessageAddress::All,
                            payload: bincode::encode_to_vec(
                                TorpedoMessage::HitOrMissReply(coord, is_hit),
                                bincode::config::standard(),
                            )
                            .expect("Failed encoding hit of miss reply message"),
                        })
                        .await
                    {
                        Ok(Response::Ok) => { /* noop */ }
                        response => {
                            panic!("Unexpected response to send message: {:?}", response);
                        }
                    }
                }
                Event::GotReplyToGuess(guess, is_hit) => {
                    // TODO - save it
                    match self.client.next_gamer().await {
                        Ok(Response::Ok) => {
                            self.cmd_queue
                                .lock()
                                .await
                                .push_front(Command::WaitForTurn(false));
                        }
                        response => {
                            panic!("Unexpected response to NEXT-GAMER: {:?}", response);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
enum Event {
    TurnIsActive(bool),
    GotGuess(Coord),
    GotReplyToGuess(Coord, bool),
}

#[derive(Debug)]
enum Command {
    Start,
    WaitForTurn(bool),
}

enum BackgroundState {
    Idle,
    WaitForTurn(bool),
}

async fn read_all_messages(client: &MGNClient) -> Vec<Message> {
    match client.fetch_all_messages().await {
        Ok(Response::OkWithMessages(messages)) => messages,
        response => {
            error!(
                "Unexpected response {:?} for fetching all messages",
                response
            );
            vec![]
        }
    }
}

async fn background_thread(
    client: MGNClient,
    event_queue: Arc<Mutex<VecDeque<Event>>>,
    cmd_queue: Arc<Mutex<VecDeque<Command>>>,
) {
    info!("Background thread has started");
    let mut state = BackgroundState::Idle;

    loop {
        for message in read_all_messages(&client).await {
            let (torpedo_message, _size): (TorpedoMessage, _) =
                bincode::decode_from_slice(&message.payload, bincode::config::standard())
                    .expect("Failed decoding message payload");

            info!("Got message: {:?}", &torpedo_message);

            match torpedo_message {
                TorpedoMessage::Guess(coord) => {
                    event_queue.lock().await.push_front(Event::GotGuess(coord));
                }
                TorpedoMessage::HitOrMissReply(coord, is_hit) => {
                    event_queue
                        .lock()
                        .await
                        .push_front(Event::GotReplyToGuess(coord, is_hit));
                }
            }
        }

        let mut _cmd_queue = cmd_queue.lock().await;
        while let Some(cmd) = _cmd_queue.pop_back() {
            info!("Got command: {:?}", &cmd);

            match cmd {
                Command::Start => match client.start_session().await {
                    Ok(Response::Ok) => info!("Session start requested"),
                    response => {
                        panic!("Unexpected response for session start: {:?}", response)
                    }
                },
                Command::WaitForTurn(is_self) => {
                    state = BackgroundState::WaitForTurn(is_self);
                }
            }
        }

        match state {
            BackgroundState::Idle => {}
            BackgroundState::WaitForTurn(expectation) => match client.is_gamer_turn().await {
                Ok(response) => match response {
                    minignetcommon::Response::OkWithBool(is_my_turn) => {
                        info!("IS-MY-TURN response: {}", is_my_turn);
                        if is_my_turn == expectation {
                            state = BackgroundState::Idle;
                            event_queue
                                .lock()
                                .await
                                .push_front(Event::TurnIsActive(is_my_turn));
                        }
                    }
                    _ => panic!("Unexpected response to IS-GAMER-TURN: {:?}", response),
                },
                Err(_) => {
                    panic!("Error while checking turn");
                }
            },
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("Torpedo starts");

    let cmd_line_args = CmdLineArgs::parse();
    let event_queue: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::new()));
    let cmd_queue: Arc<Mutex<VecDeque<Command>>> = Arc::new(Mutex::new(VecDeque::new()));

    let session_id = cmd_line_args.session_id.clone();
    let gamer_id = cmd_line_args.gamer_id.clone();

    let event_queue_clone = event_queue.clone();
    let cmd_queue_clone = cmd_queue.clone();
    let client = MGNClient::new(cmd_line_args.server, session_id, gamer_id).unwrap();
    let client_clone = client.clone();

    let mut game = Game::new(client, event_queue, cmd_queue);
    game.init().await;

    tokio::spawn(async move {
        background_thread(client_clone, event_queue_clone, cmd_queue_clone).await;
    });

    game.run().await;
}
