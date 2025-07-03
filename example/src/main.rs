extern crate log;
extern crate pretty_env_logger;

use log::info;
use minignetclient::MGNClient;
use minignetcommon::{Message, MessageAddress};

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
        .is_game_on("session_01".into())
        .await
        .expect("Failed is game on");
    dbg!(result);

    let result = client
        .is_gamer_turn("session_01".into(), "lennox".into())
        .await
        .expect("Failed is gamer turn");
    dbg!(result);

    let result = client
        .send_update("session_01".into(), "lennox".into(), b"kukulala".to_vec())
        .await
        .expect("Failed sending update");
    dbg!(result);

    let result = client
        .get_previous_round_updates("session_01".into())
        .await
        .expect("Failed getting previous round updates");
    dbg!(result);

    let result = client
        .send_message(
            "session_01".into(),
            Message {
                from: "lennox".into(),
                to: MessageAddress::One("lennox".into()),
                payload: vec![2, 6, 8],
            },
        )
        .await
        .expect("Failed sending message");
    dbg!(result);

    let result = client
        .fetch_all_messages("session_01".into(), "lennox".into())
        .await
        .expect("Failed fetching all messages");
    dbg!(result);

    let result = client
        .end_session("session_01".into())
        .await
        .expect("Failed ending session");
    dbg!(result);
}
