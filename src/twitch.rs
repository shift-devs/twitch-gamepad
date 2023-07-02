use tokio::sync::mpsc::Sender;
use tracing::info;
use twitch_irc::{
    login::StaticLoginCredentials,
    message::{PrivmsgMessage, ServerMessage},
    ClientConfig, SecureTCPTransport, TwitchIRCClient,
};

use crate::command::{self, Message, Privilege};

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

pub fn run_twitch_irc(
    channel: String,
    tx: Sender<Message>,
) -> (
    tokio::task::JoinHandle<()>,
    TwitchIRCClient<SecureTCPTransport, StaticLoginCredentials>,
) {
    let config = ClientConfig::default();
    let (mut message_stream, client) =
        TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);

    let handle = {
        tokio::spawn(async move {
            info!("Starting twitch IRC on channel {}", channel);
            while let Some(msg) = message_stream.recv().await {
                if let ServerMessage::Privmsg(msg) = msg {
                    info!("Received: {:?}", msg);
                    let privilege = user_privilege(&msg, &channel);

                    if let Some(command) = command::process_command(&msg.message_text) {
                        let msg = Message {
                            command,
                            sender_name: msg.sender.login,
                            sender_id: msg.sender.id,
                            privilege,
                        };

                        info!("Message: {:?}", msg);
                        tx.send(msg).await.unwrap();
                    }
                }
            }
        })
    };

    (handle, client)
}
