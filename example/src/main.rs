extern crate log;
extern crate pretty_env_logger;

use log::info;
use minignetclient::MGNClient;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("Server has started");

    let client = MGNClient::new("127.0.0.1:8888").expect("Failed initializing a client");

    let result = client
        .join_session("session_01".into(), "lennox".into())
        .await
        .expect("Failed joining session");
    dbg!(result);

    let result = client
        .reset_session("session_01".into())
        .await
        .expect("Failed to reset session");
    dbg!(result);

    let result = client
        .start_session("session_01".into())
        .await
        .expect("Failed starting session");
    dbg!(result);

    let result = client
        .is_gamer_turn("session_01".into(), "lennox".into())
        .await
        .expect("Failed is gamer turn");
    dbg!(result);

    let result = client
        .end_session("session_01".into())
        .await
        .expect("Failed ending session");
    dbg!(result);
}
