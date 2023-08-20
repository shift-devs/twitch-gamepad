use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::sync::{
    mpsc::{Sender, UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tracing::{error, info, trace};
use twitch_irc::{
    login::{
        LoginCredentials, RefreshingLoginCredentials, StaticLoginCredentials, TokenStorage,
        UserAccessToken,
    },
    message::{PrivmsgMessage, ServerMessage, UserNoticeEvent},
    transport::Transport,
    ClientConfig, SecureTCPTransport, TwitchIRCClient,
};

use crate::{
    command::{self, Message, Privilege},
    game_runner::SfxRequest,
};

#[derive(Debug)]
pub struct CredStore {
    path: PathBuf,
}

#[async_trait::async_trait]
impl TokenStorage for CredStore {
    type LoadError = anyhow::Error;
    type UpdateError = anyhow::Error;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        let token_str = tokio::fs::read_to_string(&self.path).await?;
        let token = toml::from_str::<UserAccessToken>(&token_str)?;
        Ok(token)
    }

    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
        let token_str = toml::to_string(token)?;
        tokio::fs::write(&self.path, &token_str).await?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

pub async fn bootstrap_tokens(
    client_id: String,
    secret: String,
    access: String,
    token_path: &Path,
) -> anyhow::Result<()> {
    info!("Bootstrapping token");
    let client = reqwest::Client::new();
    let url = reqwest::Url::parse_with_params(
        "https://id.twitch.tv/oauth2/token",
        &[
            ("client_id", client_id.as_str()),
            ("client_secret", secret.as_str()),
            ("code", access.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", "https://localhost:8080/"),
        ],
    )?;
    let resp = client.post(url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Err(anyhow::anyhow!("Failed to bootstrap token"));
    }

    let tokens: TokenResponse = resp.json().await?;
    let created_at = chrono::Utc::now();
    let expiry_time =
        created_at + chrono::Duration::from_std(std::time::Duration::new(tokens.expires_in, 0))?;
    let tokens = UserAccessToken {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        created_at: chrono::Utc::now(),
        expires_at: Some(expiry_time),
    };

    let tokens_toml = toml::to_string(&tokens)?;
    tokio::fs::write(token_path, &tokens_toml).await?;

    info!("Token bootstrap complete");

    Ok(())
}

fn is_moderator(msg: &PrivmsgMessage) -> bool {
    fn is_mod_option(msg: &PrivmsgMessage) -> Option<bool> {
        let tags = &msg.source.tags.0;
        let mod_tag = tags.get("mod")?.as_ref()?;
        Some(mod_tag == "1")
    }

    is_mod_option(msg).is_some_and(|x| x)
}

pub fn user_privilege(msg: &PrivmsgMessage, channel: &str) -> Privilege {
    if channel == msg.sender.login {
        return Privilege::Broadcaster;
    }

    if is_moderator(msg) {
        return Privilege::Moderator;
    }

    Privilege::Standard
}

async fn process_message<R>(
    tx: &mut Sender<command::WithReply<Message, R>>,
    channel: &str,
    msg: &PrivmsgMessage,
) -> Option<oneshot::Receiver<R>> {
    trace!("Received: {:?}", msg);
    let privilege = user_privilege(msg, channel);

    if let Some(command) = command::parse_command(&msg.message_text) {
        let command = Message {
            command,
            sender_name: msg.sender.login.clone(),
            sender_id: msg.sender.id.clone(),
            privilege,
        };

        info!("Command: {:?}", command);
        let (command, reply_rx) = command::WithReply::new(command);
        tx.send(command).await.unwrap();
        Some(reply_rx)
    } else {
        None
    }
}

pub async fn run_twitch_irc<T: Transport, L: LoginCredentials>(
    client: TwitchIRCClient<T, L>,
    mut stream: UnboundedReceiver<ServerMessage>,
    channel: String,
    mut tx: Sender<command::WithReply<Message, Option<String>>>,
    mut sfx_runner: Option<UnboundedSender<SfxRequest>>,
) {
    while let Some(msg) = stream.recv().await {
        match msg {
            ServerMessage::Privmsg(msg) => {
                let reply_rx = process_message(&mut tx, &channel, &msg).await;
                let reply_rx = if let Some(reply_rx) = reply_rx {
                    reply_rx
                } else {
                    continue;
                };

                if let Ok(Some(response)) = reply_rx.await {
                    info!("Response: {}", response);
                    if let Err(err) = client.say_in_reply_to(&msg, response).await {
                        error!("Error replying to twitch message: {:?}", err);
                    }
                }
            }
            ServerMessage::UserNotice(notice) => {
                info!("Received rich event {:?}", notice);
                let sfx_runner: &mut UnboundedSender<SfxRequest> = match sfx_runner {
                    Some(ref mut x) => x,
                    None => continue,
                };

                let event = match notice.event {
                    UserNoticeEvent::SubMysteryGift {
                        mass_gift_count, ..
                    } => Some(SfxRequest::SubEvent(mass_gift_count)),
                    UserNoticeEvent::AnonSubMysteryGift {
                        mass_gift_count, ..
                    } => Some(SfxRequest::SubEvent(mass_gift_count)),
                    _ => None,
                };

                if let Some(effect) = event {
                    info!("Sending effect {:?}", effect);
                    if let Err(e) = sfx_runner.send(effect) {
                        error!("Unable to send sfx event: {:?}", e);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn run_twitch_irc_login(
    client: String,
    secret: String,
    token_path: &Path,
    channel: String,
    tx: Sender<command::WithReply<Message, Option<String>>>,
    sfx_runner: Option<UnboundedSender<SfxRequest>>,
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
    let store = CredStore {
        path: token_path.to_owned(),
    };
    let credentials = RefreshingLoginCredentials::init(client, secret, store);

    let config = ClientConfig::new_simple(credentials);
    let (message_stream, client) =
        TwitchIRCClient::<SecureTCPTransport, RefreshingLoginCredentials<CredStore>>::new(config);

    let runner_handle = {
        let client = client.clone();
        let channel = channel.clone();
        tokio::spawn(async move {
            info!("Starting twitch IRC on channel {}", channel);
            run_twitch_irc(client, message_stream, channel, tx, sfx_runner).await;
        })
    };

    let client_join_handle = tokio::task::spawn(async move { client.join(channel).unwrap() });
    (runner_handle, client_join_handle)
}

pub fn run_twitch_irc_anonymous(
    channel: String,
    tx: Sender<command::WithReply<Message, Option<String>>>,
    sfx_runner: Option<UnboundedSender<SfxRequest>>,
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
    let config = ClientConfig::default();
    let (message_stream, client) =
        TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);

    let runner_handle = {
        let client = client.clone();
        let channel = channel.clone();
        tokio::spawn(async move {
            info!("Starting twitch IRC on channel {}", channel);
            run_twitch_irc(client, message_stream, channel, tx, sfx_runner).await;
        })
    };

    let client_join_handle = tokio::task::spawn(async move { client.join(channel).unwrap() });
    (runner_handle, client_join_handle)
}
