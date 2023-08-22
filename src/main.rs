use command::Message;
use tokio::{self, io::AsyncBufReadExt};
use twitch::run_twitch_irc_login;

mod command;
mod config;
mod database;
mod game_runner;
mod gamepad;
mod twitch;

#[cfg(test)]
mod test;

fn stdin_input(
    tx: tokio::sync::mpsc::Sender<command::WithReply<Message, Option<String>>>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::task::spawn(async move {
        loop {
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
                    let (msg, reply_rx) = command::WithReply::new(msg);
                    tx.send(msg).await?;
                    if let Ok(Some(reply)) = reply_rx.await {
                        tracing::info!("Reply: {:?}", reply);
                    }
                }

                line.clear();
            }
        }
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();
    if let Err(std::env::VarError::NotPresent) = std::env::var("DISPLAY") {
        tracing::error!("Cannot find graphical display env vars, bailing");
        std::process::exit(1);
    }

    let (config, cfg_path) = config::read_config().await.unwrap();
    let cfg_dir = cfg_path.parent().unwrap();
    let channel = &config.twitch.channel_name;

    let db_path = cfg_dir.join("twitch_gamepad.db");
    let db_conn = database::connect(&db_path).unwrap();

    let (_, mut sfx_tx) = match config
        .sound_effects
        .clone()
        .map(game_runner::run_sfx_runner)
    {
        Some((x, y)) => (Some(x), Some(y)),
        None => (None, None),
    };

    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let (_, client_handle) = match &config.twitch.auth {
        config::TwitchAuth::Anonymous => {
            twitch::run_twitch_irc_anonymous(channel.clone(), tx.clone(), sfx_tx.clone())
        }
        config::TwitchAuth::Login {
            client,
            secret,
            access,
        } => {
            let token_path = cfg_dir.join("tokens.toml");
            if !token_path.exists() && access.is_none() {
                tracing::error!(
                    "Must seed tokens in {:?} before using login auth",
                    token_path
                );
                tracing::error!("Visit https://id.twitch.tv/oauth2/authorize?client_id={}&response_type=code&scope=chat%3Aedit+chat%3Aread&redirect_uri=https://localhost%3A8080/ to obtain initial keys, then set 'access' in twitch.auth.credentials to the returned code", client);
                return;
            }

            if !token_path.exists() && access.is_some() {
                twitch::bootstrap_tokens(
                    client.clone(),
                    secret.clone(),
                    access.clone().unwrap(),
                    &token_path,
                )
                .await
                .unwrap();
            }

            run_twitch_irc_login(
                client.clone(),
                secret.clone(),
                &token_path,
                channel.clone(),
                tx.clone(),
                sfx_tx.clone(),
            )
        }
    };

    stdin_input(tx.clone());

    let gamepad = gamepad::UinputGamepad::new().unwrap();
    client_handle.await.unwrap();

    let (mut gamepad_handle, gamepad_tx) = gamepad::run_gamepad(gamepad);
    let (mut game_runner_handle, game_runner_tx) = game_runner::run_game_runner();

    let command_runner: tokio::task::JoinHandle<anyhow::Result<()>> =
        tokio::task::spawn(async move {
            let mut rx = rx;
            let config = config;
            let mut db_conn = db_conn;
            let mut game_runner_tx = game_runner_tx;

            command::run_commands(
                &mut rx,
                &config,
                gamepad_tx,
                &mut db_conn,
                &mut game_runner_tx,
                sfx_tx.as_mut(),
            )
            .await?;

            Ok(())
        });

    tokio::select! {
        cr = command_runner => {
            cr.unwrap().unwrap();
            let _ = tokio::join!(gamepad_handle, game_runner_handle);
        }
        gh = &mut gamepad_handle => {
            gh.unwrap().unwrap();
        }
        grh = &mut game_runner_handle => grh.unwrap().unwrap(),
    }

    tracing::info!("Command runner finished");
    std::process::exit(0);
}
