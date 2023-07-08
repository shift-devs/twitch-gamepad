use crate::{database, gamepad::Gamepad};
use anyhow::{anyhow, Context};
use core::time::Duration;
use rusqlite::Connection;
use tokio::sync::{mpsc::Receiver, oneshot};
use tracing::info;

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Eq, Ord)]
pub enum Privilege {
    Standard = 0,
    Operator = 1,
    Moderator = 2,
    Broadcaster = 3,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub command: Command,
    pub sender_id: String,
    pub sender_name: String,
    pub privilege: Privilege,
}

#[derive(Debug)]
pub struct WithReply<T, R> {
    pub message: T,
    pub reply_tx: oneshot::Sender<R>,
}

impl<T, R> WithReply<T, R> {
    pub fn new(message: T) -> (Self, oneshot::Receiver<R>) {
        let (reply_tx, reply_rx) = oneshot::channel();
        let with_reply = Self { message, reply_tx };

        (with_reply, reply_rx)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Movement {
    A,
    B,
    C,
    X,
    Y,
    Z,
    TL,
    TR,
    Up,
    Down,
    Left,
    Right,
    Start,
    Select,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    Movement(Movement, u32),
    AddOperator(String),
    RemoveOperator(String),
    Block(String, Option<chrono::DateTime<chrono::Utc>>),
    Unblock(String),
}

fn parse_movement(tokens: &Vec<&str>) -> Option<Command> {
    if tokens.is_empty() || tokens.len() > 2 {
        return None;
    }

    let movement = match tokens[0] {
        "a" => Movement::A,
        "b" => Movement::B,
        "c" => Movement::C,
        "x" => Movement::X,
        "y" => Movement::Y,
        "z" => Movement::Z,
        "tl" => Movement::TL,
        "tr" => Movement::TR,
        "up" => Movement::Up,
        "down" => Movement::Down,
        "left" => Movement::Left,
        "right" => Movement::Right,
        "start" => Movement::Start,
        "select" => Movement::Select,
        _ => return None,
    };

    let duration_ms = match tokens.get(1) {
        Some(token) => str::parse::<u32>(token)
            .ok()
            .filter(|sec| *sec <= 5)
            .map(|sec| sec * 1000),
        None => Some(500),
    };

    duration_ms.map(|duration_ms| Command::Movement(movement, duration_ms))
}

pub fn parse_command(input: &str) -> Option<Command> {
    let mut tokens: Vec<String> = input.split_whitespace().map(|t| t.to_lowercase()).collect();
    tokens.retain(|token| *token != "\u{e0000}");

    let tokens: Vec<&str> = tokens.iter().map(|t| t.as_str()).collect();
    if let Some(cmd) = parse_movement(&tokens) {
        return Some(cmd);
    }

    match &tokens[..] {
        ["tp", "block", target] => Some(Command::Block(target.to_string(), None)),
        ["tp", "block", target, duration] => duration_str::parse(duration)
            .ok()
            .and_then(|d| chrono::Duration::from_std(d).ok())
            .map(|d| chrono::Utc::now() + d)
            .map(|d| Command::Block(target.to_string(), Some(d))),
        ["tp", "unblock", target] => Some(Command::Unblock(target.to_string())),
        ["tp", "op", target] => Some(Command::AddOperator(target.to_string())),
        ["tp", "deop", target] => Some(Command::RemoveOperator(target.to_string())),
        _ => None,
    }
}

pub async fn run_commands<G: Gamepad + Sized>(
    rx: &mut Receiver<WithReply<Message, Option<String>>>,
    gamepad: &mut G,
    db_conn: &mut Connection,
) -> anyhow::Result<()> {
    while let Some(msg) = rx.recv().await {
        use Command::*;

        let reply_tx = msg.reply_tx;
        let msg = msg.message;

        database::update_user(db_conn, &msg.sender_id, &msg.sender_name)
            .context("Failed to update user")?;

        let msg = if msg.privilege < Privilege::Operator
            && database::is_operator(db_conn, &msg.sender_id)
                .context("Failed to check for operator")?
        {
            Message {
                sender_name: msg.sender_name,
                sender_id: msg.sender_id,
                command: msg.command,
                privilege: Privilege::Operator,
            }
        } else {
            msg
        };

        match msg.command {
            Movement(movement, duration) => {
                if !database::is_blocked(db_conn, &msg.sender_id)
                    .context("Failed to check for blocked user")?
                {
                    info!("Sending movement {:?}", msg.command);
                    gamepad.press(&movement)?;
                    tokio::time::sleep(Duration::from_millis(duration as u64)).await;
                    gamepad.release(&movement)?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                } else {
                    info!("Blocked movement from {}", msg.sender_name);
                }

                reply_tx
                    .send(None)
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            AddOperator(user) => {
                if msg.privilege >= Privilege::Moderator {
                    database::op_user(db_conn, &user).context("Failed to op user")?;
                    info!("Added {} as operator", user);

                    reply_tx
                        .send(Some(format!("Added {} as operator", user)))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to add operator {} with insufficient privilege {:?}",
                        msg.sender_name, user, msg.privilege
                    );

                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            RemoveOperator(user) => {
                if msg.privilege >= Privilege::Moderator {
                    database::deop_user(db_conn, &user).context("Failed to deop user")?;
                    info!("Removed {} as operator", user);

                    reply_tx
                        .send(Some(format!("Removed {} as operator", user)))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to remove operator {} with insufficient privilege {:?}",
                        msg.sender_name, user, msg.privilege
                    );

                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            Block(user, duration) => {
                if msg.privilege >= Privilege::Moderator {
                    database::block_user(db_conn, &user, duration)
                        .context("Failed to block user")?;
                    info!("Blocked user {} until time {:?}", user, duration);

                    reply_tx
                        .send(Some(format!(
                            "Blocked {} {}",
                            user,
                            if let Some(duration) = duration {
                                format!("until {}", duration)
                            } else {
                                "forever".to_owned()
                            }
                        )))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to block {} until {:?} with insufficient privilege {:?}",
                        msg.sender_name, user, duration, msg.privilege
                    );

                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            Unblock(user) => {
                if msg.privilege >= Privilege::Moderator {
                    database::unblock_user(db_conn, &user).context("Failed to unblock user")?;
                    info!("Unblocked user {}", user);

                    reply_tx
                        .send(Some(format!("Unblocked {}", user)))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to unblock {} with insufficient privilege {:?}",
                        msg.sender_name, user, msg.privilege
                    );

                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod parsing_test {
    use super::{parse_command, Command, Movement};

    macro_rules! test_command {
        ($id: ident, $cmd: expr, $result: expr) => {
            #[test]
            fn $id() {
                assert_eq!(parse_command($cmd), $result);
            }
        };
    }

    test_command!(
        parse_movement_sensitivity,
        "A",
        Some(Command::Movement(Movement::A, 500))
    );
    test_command!(
        parse_movement_a,
        "a",
        Some(Command::Movement(Movement::A, 500))
    );
    test_command!(
        parse_movement_b,
        "b",
        Some(Command::Movement(Movement::B, 500))
    );
    test_command!(
        parse_movement_c,
        "c",
        Some(Command::Movement(Movement::C, 500))
    );
    test_command!(
        parse_movement_x,
        "x",
        Some(Command::Movement(Movement::X, 500))
    );
    test_command!(
        parse_movement_y,
        "y",
        Some(Command::Movement(Movement::Y, 500))
    );
    test_command!(
        parse_movement_z,
        "z",
        Some(Command::Movement(Movement::Z, 500))
    );
    test_command!(
        parse_movement_tl,
        "tl",
        Some(Command::Movement(Movement::TL, 500))
    );
    test_command!(
        parse_movement_tr,
        "tr",
        Some(Command::Movement(Movement::TR, 500))
    );
    test_command!(
        parse_movement_start,
        "start",
        Some(Command::Movement(Movement::Start, 500))
    );
    test_command!(
        parse_movement_select,
        "select",
        Some(Command::Movement(Movement::Select, 500))
    );
    test_command!(
        parse_movement_up,
        "up",
        Some(Command::Movement(Movement::Up, 500))
    );
    test_command!(
        parse_movement_down,
        "down",
        Some(Command::Movement(Movement::Down, 500))
    );
    test_command!(
        parse_movement_left,
        "left",
        Some(Command::Movement(Movement::Left, 500))
    );
    test_command!(
        parse_movement_right,
        "right",
        Some(Command::Movement(Movement::Right, 500))
    );
    test_command!(
        parse_movement_duration,
        "a 2",
        Some(Command::Movement(Movement::A, 2000))
    );

    test_command!(
        parse_block,
        "tp block user",
        Some(Command::Block("user".to_string(), None))
    );
    test_command!(
        parse_unblock,
        "tp unblock user",
        Some(Command::Unblock("user".to_string()))
    );
    test_command!(
        parse_op,
        "tp op user",
        Some(Command::AddOperator("user".to_string()))
    );
    test_command!(
        parse_deop,
        "tp deop user",
        Some(Command::RemoveOperator("user".to_string()))
    );

    test_command!(parse_invalid, "asdf", None);
    test_command!(
        parse_extraneous,
        "tp block user 3m and then something",
        None
    );
    test_command!(parse_movement_extraneous, "a 2 and something", None);
    test_command!(parse_movement_invalid_time, "a b", None);
    test_command!(parse_movement_time_too_large, "a 100", None);
    test_command!(
        parse_twitch_deduplicated,
        "a \u{e0000}",
        Some(Command::Movement(Movement::A, 500))
    );
    test_command!(parse_block_invalid_time, "tp block user notatime", None);

    #[test]
    fn parse_block_duration() {
        let cmd = parse_command("tp block user 1h3m").unwrap();
        let expected_duration =
            chrono::Duration::from_std(std::time::Duration::new(3600 + 60 * 3, 0)).unwrap();
        let curtime = chrono::Utc::now();
        if let Command::Block(username, time) = cmd {
            let time = time.expect("did not parse duration");
            let duration = time - curtime;
            let duration = expected_duration - duration;
            assert_eq!(duration.num_seconds(), 0);
            assert_eq!(username, "user");
        } else {
            unreachable!("Not a block command");
        }
    }
}
