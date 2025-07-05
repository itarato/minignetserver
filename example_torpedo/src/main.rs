extern crate log;
extern crate pretty_env_logger;

use bincode::{Decode, Encode};
use log::{error, info, warn};
use std::io::Write;
use std::{collections::VecDeque, sync::Arc, time::Duration};

use clap::Parser;
use minignetclient::MGNClient;
use minignetcommon::{Error, GamerIdType, Message, MessageAddress, Response, SessionIdType};
use rand::{prelude::*, rng};
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

const SHIP_SIZES: [u8; 5] = [5, 4, 3, 3, 2];
const DIR_MAP: [[u8; 2]; 2] = [[1, 0], [0, 1]];

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

#[derive(Debug, Decode, Encode, PartialEq)]
struct Coord {
    x: u8,
    y: u8,
}

impl Coord {
    fn singular(&self) -> usize {
        (self.y * 10 + self.x) as usize
    }
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

#[derive(Debug)]
enum Event {
    StartRequest,
    GameIsActive,
    TurnIsActive(bool),
    MakeGuess(Coord),
    GotGuess(Coord),
    GotReplyToGuess(Coord, bool),
}

#[derive(Debug)]
enum Command {
    WaitForGameOn,
    WaitForTurn(bool),
}

enum BackgroundState {
    Idle,
    WaitForGameOn,
    WaitForTurn(bool),
}

#[derive(Debug, Clone, PartialEq)]
enum GameState {
    Init,
    SelfTurn,
    OtherTurn,
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
            let y = caps.get(1).unwrap().as_str().chars().next().unwrap() as u8 - b'a';
            let x: u8 = caps.get(2).unwrap().as_str().parse().unwrap();
            return Ok(InputCommand::Step(Coord { x: x - 1, y: y }));
        }

        Err(format!("Unparsable command: {:?}", raw).into())
    }
}

struct Game {
    self_board: [CellState; 100],
    other_board: [CellState; 100],
    ship_coords: Vec<Coord>,
    client: MGNClient,
    event_queue: Arc<Mutex<VecDeque<Event>>>,
    cmd_queue: Arc<Mutex<VecDeque<Command>>>,
    state: GameState,
}

impl Game {
    fn new(
        client: MGNClient,
        event_queue: Arc<Mutex<VecDeque<Event>>>,
        cmd_queue: Arc<Mutex<VecDeque<Command>>>,
    ) -> Self {
        let mut ship_coords = vec![];
        for ship_size in SHIP_SIZES {
            loop {
                let startx: u8 = rng().random_range(1..=10);
                let starty: u8 = rng().random_range(1..=10);
                let dir: usize = rng().random_range(0..=1);

                let mut is_fit = true;
                let mut new_ship_coords = vec![];
                for i in 0..ship_size {
                    let x = startx + DIR_MAP[dir][0] * i;
                    let y = starty + DIR_MAP[dir][1] * i;
                    let coord = Coord { x, y };

                    if ship_coords.contains(&coord) || x >= 10 || y >= 10 {
                        is_fit = false;
                        break;
                    }

                    new_ship_coords.push(coord);
                }

                if !is_fit {
                    continue;
                }

                info!("Ship at: x={} y={} d={}", startx, starty, dir);
                ship_coords.append(&mut new_ship_coords);
                break;
            }
        }

        Self {
            self_board: [CellState::Undiscovered; 100],
            other_board: [CellState::Undiscovered; 100],
            ship_coords,
            client,
            event_queue,
            cmd_queue,
            state: GameState::Init,
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
            .push_front(Command::WaitForGameOn);
    }

    async fn run(&mut self) {
        self.refresh_screen();

        loop {
            self.consume_events().await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn consume_events(&mut self) {
        loop {
            let event_popped = self.event_queue.lock().await.pop_back();

            if let Some(event) = event_popped {
                info!("Got event: {:?}", &event);

                match event {
                    Event::StartRequest => {
                        if self.state != GameState::Init {
                            warn!("Cannot start game, already started.");
                            continue;
                        }

                        self.state = GameState::OtherTurn;

                        match self.client.start_session().await {
                            Ok(Response::Ok) => info!("Session start requested"),
                            response => {
                                panic!("Unexpected response for session start: {:?}", response)
                            }
                        };
                    }
                    Event::GameIsActive => {
                        self.cmd_queue
                            .lock()
                            .await
                            .push_front(Command::WaitForTurn(true));
                    }
                    Event::TurnIsActive(is_self) => {
                        if !is_self {
                            self.state = GameState::OtherTurn;
                            self.cmd_queue
                                .lock()
                                .await
                                .push_front(Command::WaitForTurn(true));
                        } else {
                            self.state = GameState::SelfTurn;
                        }
                    }
                    Event::GotGuess(coord) => {
                        let is_hit = self.ship_coords.contains(&coord);

                        self.self_board[coord.singular()] = if is_hit {
                            CellState::Hit
                        } else {
                            CellState::Miss
                        };

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
                        self.other_board[guess.singular()] = if is_hit {
                            CellState::Hit
                        } else {
                            CellState::Miss
                        };

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
                    Event::MakeGuess(coord) => {
                        if self.state != GameState::SelfTurn {
                            warn!("Cannot guess while not on turn");
                            continue;
                        }

                        match self
                            .client
                            .send_message(Message {
                                from: self.client.gamer_id.clone(),
                                to: MessageAddress::All,
                                payload: bincode::encode_to_vec(
                                    TorpedoMessage::Guess(coord),
                                    bincode::config::standard(),
                                )
                                .expect("Failed encoding guess"),
                            })
                            .await
                        {
                            Ok(Response::Ok) => { /* noop */ }
                            response => {
                                panic!("Unexpected respone to SEND-MESSAGE: {:?}", response)
                            }
                        }
                    }
                }

                self.refresh_screen();
            } else {
                break;
            }
        }
    }

    fn refresh_screen(&self) {
        print!("\x1B[2J\x1B[1;1H");

        println!(
            "\x1B[93m\x1B[1m   SELF                                              OTHER\x1B[0m"
        );
        println!(
            "\x1B[93m   1   2   3   4   5   6   7   8   9   10            1   2   3   4   5   6   7   8   9   10\x1B[0m"
        );

        for y in 0..10 {
            print!("\x1B[93m{}\x1B[0m ", (b'A' + (y as u8)) as char);

            for x in 0..10 {
                if self.ship_coords.contains(&Coord { x, y }) {
                    print!("\x1B[38;5;208m[\x1B[0m");
                } else {
                    print!("\x1B[90m[\x1B[0m");
                }

                match self.self_board[(y * 10 + x) as usize] {
                    CellState::Undiscovered => print!(" "),
                    CellState::Hit => print!("\x1B[91m█\x1B[0m"),
                    CellState::Miss => print!("\x1B[97m░\x1B[0m"),
                }

                if self.ship_coords.contains(&Coord { x, y }) {
                    print!("\x1B[38;5;208m] \x1B[0m");
                } else {
                    print!("\x1B[90m] \x1B[0m");
                }
            }

            print!("        ");

            print!("\x1B[93m{}\x1B[0m ", (b'A' + (y as u8)) as char);
            for x in 0..10 {
                print!("\x1B[90m[\x1B[0m");
                match self.other_board[(y * 10 + x) as usize] {
                    CellState::Undiscovered => print!(" "),
                    CellState::Hit => print!("\x1B[91m█\x1B[0m"),
                    CellState::Miss => print!("\x1B[97m░\x1B[0m"),
                }
                print!("\x1B[90m] \x1B[0m");
            }

            print!("\n\n");
        }

        match self.state {
            GameState::Init => print!("Type 'start' to start > "),
            GameState::SelfTurn => print!("Guess > "),
            GameState::OtherTurn => print!("... wait for the other player ..."),
        }

        std::io::stdout().flush().unwrap();
    }
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
                Command::WaitForTurn(is_self) => state = BackgroundState::WaitForTurn(is_self),
                Command::WaitForGameOn => state = BackgroundState::WaitForGameOn,
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
                Err(_) => panic!("Error while checking turn"),
            },
            BackgroundState::WaitForGameOn => match client.is_game_on().await {
                Ok(response) => match response {
                    minignetcommon::Response::OkWithBool(is_game_on) => {
                        info!("IS-GAME-ON response: {}", is_game_on);
                        if is_game_on {
                            event_queue.lock().await.push_front(Event::GameIsActive);
                        }
                    }
                    _ => panic!("Unexpected response to IS-GAME-ON: {:?}", response),
                },
                Err(_) => panic!("Error while checking turn"),
            },
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn stdin_readline_thread(event_queue: Arc<Mutex<VecDeque<Event>>>) {
    loop {
        let mut stdin = BufReader::new(io::stdin()).lines();

        match stdin.next_line().await {
            Ok(Some(line)) => match InputParser::parse(line) {
                Ok(cmd) => match cmd {
                    InputCommand::Start => event_queue.lock().await.push_front(Event::StartRequest),
                    InputCommand::Step(coord_guess) => event_queue
                        .lock()
                        .await
                        .push_front(Event::MakeGuess(coord_guess)),
                },
                Err(err) => {
                    error!("Cannot parse command: {:?}", err);
                }
            },
            error => warn!("Unexpected result at line reading: {:?}", error),
        }
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

    let mut game = Game::new(client, event_queue.clone(), cmd_queue.clone());
    game.init().await;

    tokio::spawn(async move {
        background_thread(client_clone, event_queue_clone, cmd_queue_clone).await;
    });

    let event_queue_clone = event_queue.clone();
    tokio::spawn(async move {
        stdin_readline_thread(event_queue_clone).await;
    });

    game.run().await;
}
