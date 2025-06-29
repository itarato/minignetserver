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
        .join_session("lennox".into())
        .await
        .expect("Failed joining session");

    dbg!(result);
}
