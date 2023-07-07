use command::Message;
use tokio::{self, io::AsyncBufReadExt};

mod command;
mod config;
mod database;
mod gamepad;
mod twitch;

#[cfg(test)]
mod test;

fn stdin_input(tx: tokio::sync::mpsc::Sender<Message>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
        let mut line = String::new();

        while let Ok(sz) = reader.read_line(&mut line).await {
            if sz == 0 {
                break;
            }

            if let Some(cmd) = command::parse_command(&line) {
                let msg = command::Message {
                    command: cmd,
                    sender_name: "stdin".to_owned(),
                    sender_id: "stdin".to_owned(),
                    privilege: command::Privilege::Broadcaster,
                };

                tracing::info!("Message: {:?}", msg);
                tx.send(msg).await.unwrap();
            }

            line.clear();
        }
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();
    let (config, cfg_path) = config::read_config().await.unwrap();
    let channel = config.twitch.channel_name;

    let db_path = cfg_path.parent().unwrap().join("twitch_gamepad.db");
    let mut db_conn = database::connect(&db_path).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let (msg_join_handle, client) = twitch::run_twitch_irc(channel.clone(), tx.clone());

    let stdin_join_handle = stdin_input(tx.clone());

    let mut gamepad = gamepad::UinputGamepad::new().unwrap();
    client.join(channel).unwrap();

    command::run_commands(&mut rx, &mut gamepad, &mut db_conn)
        .await
        .unwrap();

    msg_join_handle.await.unwrap();
    stdin_join_handle.await.unwrap();
}
