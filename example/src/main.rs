extern crate log;
extern crate pretty_env_logger;

use log::info;
use minignetclient::MGNClient;
use minignetcommon::{Message, MessageAddress};

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("Server has started");

    let client = MGNClient::new("127.0.0.1:8888", "session_01".into(), "lennox".into())
        .expect("Failed initializing a client");

    let result = client.join_session().await.expect("Failed joining session");
    dbg!(result);

    let result = client
        .reset_session()
        .await
        .expect("Failed to reset session");
    dbg!(result);

    let result = client
        .start_session()
        .await
        .expect("Failed starting session");
    dbg!(result);

    let result = client.is_game_on().await.expect("Failed is game on");
    dbg!(result);

    let result = client.is_gamer_turn().await.expect("Failed is gamer turn");
    dbg!(result);

    let result = client
        .send_update(b"kukulala".to_vec())
        .await
        .expect("Failed sending update");
    dbg!(result);

    let result = client
        .get_previous_round_updates()
        .await
        .expect("Failed getting previous round updates");
    dbg!(result);

    let result = client
        .send_message(Message {
            from: "lennox".into(),
            to: MessageAddress::One("lennox".into()),
            payload: vec![2, 6, 8],
        })
        .await
        .expect("Failed sending message");
    dbg!(result);

    let result = client
        .fetch_all_messages()
        .await
        .expect("Failed fetching all messages");
    dbg!(result);

    let result = client.end_session().await.expect("Failed ending session");
    dbg!(result);
}
