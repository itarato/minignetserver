extern crate log;
extern crate pretty_env_logger;

use clap::builder::Str;
use log::{error, warn};
use std::{collections::VecDeque, sync::Arc, time::Duration};

use clap::Parser;
use minignetclient::MGNClient;
use minignetcommon::{Error, GamerIdType, Response, SessionIdType};
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
    SelfTurn,
    OtherTurn,
}

struct Coord {
    x: u8,
    y: u8,
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

        Err(format!("Unparsable command: {:?}", raw).into())
    }
}

struct Game {
    self_board: [CellState; 100],
    other_board: [CellState; 100],
    state: GameState,
    client: MGNClient,
    session_id: SessionIdType,
    gamer_id: GamerIdType,
    event_queue: Arc<Mutex<VecDeque<Event>>>,
    cmd_queue: Arc<Mutex<VecDeque<Command>>>,
}

impl Game {
    fn new(
        client: MGNClient,
        session_id: SessionIdType,
        gamer_id: GamerIdType,
        event_queue: Arc<Mutex<VecDeque<Event>>>,
        cmd_queue: Arc<Mutex<VecDeque<Command>>>,
    ) -> Self {
        Self {
            self_board: [CellState::Undiscovered; 100],
            other_board: [CellState::Undiscovered; 100],
            state: GameState::Initialize,
            client,
            session_id,
            gamer_id,
            event_queue,
            cmd_queue,
        }
    }

    async fn run_init(&mut self) {
        self.cmd_queue
            .lock()
            .await
            .push_front(Command::WaitForTurn(true));

        loop {
            self.execute_command_if_any().await;
            self.consume_events().await;

            match self.state {
                GameState::Initialize => {
                    match self.client.is_game_on(self.session_id.clone()).await {
                        Ok(Response::OkWithBool(is_game_on)) => {
                            if is_game_on {
                                self.state = GameState::OtherTurn;
                            }
                        }
                        response => {
                            warn!("Unexpected response for IS_GAME_ON: {:?}", response);
                        }
                    }
                }
                GameState::SelfTurn => {
                    self.cmd_queue
                        .lock()
                        .await
                        .push_front(Command::WaitForTurn(false));
                }
                GameState::OtherTurn => {
                    match self
                        .client
                        .is_gamer_turn(self.session_id.clone(), self.gamer_id.clone())
                        .await
                    {
                        Ok(Response::OkWithBool(is_gamer_turn)) => {
                            if is_gamer_turn {
                                self.state = GameState::SelfTurn;
                            }
                        }
                        response => {
                            warn!("Unexpected response for IS_GAMER_TURN: {:?}", response)
                        }
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        unimplemented!()
    }

    async fn read_stdin_line() -> Option<String> {
        let mut stdin = BufReader::new(io::stdin()).lines();

        match stdin.next_line().await {
            Ok(line) => line,
            _ => None,
        }
    }

    async fn execute_command_if_any(&mut self) -> bool {
        if let Some(stdin_line) = Game::read_stdin_line().await {
            match InputParser::parse(stdin_line) {
                Ok(cmd) => match cmd {
                    InputCommand::Start => self.cmd_queue.lock().await.push_front(Command::Start),
                    InputCommand::Step(Coord { x, y }) => unimplemented!(),
                },
                Err(err) => {
                    error!("Cannot parse command: {:?}", err);
                }
            }
        }

        false
    }

    async fn consume_events(&mut self) {
        let mut _event_queue = self.event_queue.lock().await;
        while let Some(event) = _event_queue.pop_back() {
            match event {
                Event::TurnIsActive(is_self) => {
                    if is_self {
                        self.state = GameState::SelfTurn;
                    } else {
                        self.state = GameState::OtherTurn;
                    }
                }
            }
        }
    }
}

enum Event {
    TurnIsActive(bool),
}

enum Command {
    Start,
    WaitForTurn(bool),
}

enum BackgroundState {
    WaitingForCommand,
    WaitForTurn(bool),
}

async fn background_thread(
    client: MGNClient,
    session_id: SessionIdType,
    gamer_id: GamerIdType,
    event_queue: Arc<Mutex<VecDeque<Event>>>,
    cmd_queue: Arc<Mutex<VecDeque<Command>>>,
) {
    let mut state = BackgroundState::WaitingForCommand;

    loop {
        match state {
            BackgroundState::WaitingForCommand => {
                let mut _cmd_queue = cmd_queue.lock().await;
                while let Some(cmd) = _cmd_queue.pop_back() {
                    match cmd {
                        Command::Start => {
                            if let Err(err) = client.start_session(session_id.clone()).await {
                                error!("Cannot start session: {:?}", err);
                            }
                        }
                        Command::WaitForTurn(is_self) => {
                            state = BackgroundState::WaitForTurn(is_self);
                        }
                    }
                }
            }
            BackgroundState::WaitForTurn(is_self) => {
                match client
                    .is_gamer_turn(session_id.clone(), gamer_id.clone())
                    .await
                {
                    Ok(response) => match response {
                        minignetcommon::Response::OkWithBool(is_my_turn) => {
                            if is_my_turn == is_self {
                                state = BackgroundState::WaitingForCommand;
                                let mut _event_queue = event_queue.lock().await;
                                _event_queue.push_front(Event::TurnIsActive(is_my_turn));
                            }
                        }
                        _ => warn!("Unexpected response: {:?}", response),
                    },
                    Err(_) => {
                        error!("Error while checking turn");
                        // ??? Should we send an error-event to the game?
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let cmd_line_args = CmdLineArgs::parse();
    let event_queue: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::new()));
    let cmd_queue: Arc<Mutex<VecDeque<Command>>> = Arc::new(Mutex::new(VecDeque::new()));

    let session_id = cmd_line_args.session_id.clone();
    let gamer_id = cmd_line_args.gamer_id.clone();

    let event_queue_clone = event_queue.clone();
    let cmd_queue_clone = cmd_queue.clone();
    let client = MGNClient::new(cmd_line_args.server).unwrap();
    let client_clone = client.clone();

    tokio::spawn(async move {
        background_thread(
            client_clone,
            session_id,
            gamer_id,
            event_queue_clone,
            cmd_queue_clone,
        )
    });

    Game::new(
        client,
        cmd_line_args.session_id,
        cmd_line_args.gamer_id,
        event_queue,
        cmd_queue,
    )
    .run_init()
    .await;
}
