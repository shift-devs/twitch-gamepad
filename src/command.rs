use crate::{database, gamepad::Gamepad};
use anyhow::Context;
use core::time::Duration;
use rusqlite::Connection;
use tokio::sync::mpsc::Receiver;
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

#[derive(Copy, Clone, Debug)]
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

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Command {
    Movement(Movement, u32),
    AddOperator(String),
    RemoveOperator(String),
    Block(String, Option<chrono::DateTime<chrono::Utc>>),
    Unblock(String),
}

fn process_movement(tokens: &Vec<String>) -> Option<Command> {
    if tokens.len() > 2 {
        return None;
    }

    let movement = match tokens[0].as_str() {
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

pub fn process_command(input: &str) -> Option<Command> {
    let mut tokens: Vec<String> = input.split_whitespace().map(|t| t.to_lowercase()).collect();
    tokens.retain(|token| token != "\u{e0000}");
    if tokens.is_empty() {
        return None;
    }

    if let Some(cmd) = process_movement(&tokens) {
        return Some(cmd);
    }

    if tokens[0].as_str() != "tp" {
        return None;
    }

    match tokens.get(1).map(|t| t.as_str()) {
        Some("block") => {
            let duration = tokens
                .get(3)
                .and_then(|t| duration_str::parse(t).ok())
                .and_then(|d| chrono::Duration::from_std(d).ok())
                .map(|d| chrono::Utc::now() + d);
            tokens
                .get(2)
                .map(move |target| Command::Block(target.clone(), duration))
        }
        Some("unblock") => tokens.get(2).map(|target| Command::Unblock(target.clone())),
        Some("op") => tokens
            .get(2)
            .map(|target| Command::AddOperator(target.clone())),
        Some("deop") => tokens
            .get(2)
            .map(|target| Command::RemoveOperator(target.clone())),
        _ => None,
    }
}

pub async fn run_commands<G: Gamepad + Sized>(
    mut rx: Receiver<Message>,
    gamepad: &mut G,
    db_conn: &mut Connection,
) -> anyhow::Result<()> {
    while let Some(msg) = rx.recv().await {
        use Command::*;

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
            }
            AddOperator(user) => {
                if msg.privilege >= Privilege::Moderator {
                    database::op_user(db_conn, &user).context("Failed to op user")?;
                    info!("Added {} as operator", user);
                } else {
                    info!(
                        "{} attempted to add operator {} with insufficient privilege {:?}",
                        msg.sender_name, user, msg.privilege
                    );
                }
            }
            RemoveOperator(user) => {
                if msg.privilege >= Privilege::Moderator {
                    database::deop_user(db_conn, &user).context("Failed to deop user")?;
                    info!("Removed {} as operator", user);
                } else {
                    info!(
                        "{} attempted to remove operator {} with insufficient privilege {:?}",
                        msg.sender_name, user, msg.privilege
                    );
                }
            }
            Block(user, duration) => {
                if msg.privilege >= Privilege::Moderator {
                    database::block_user(db_conn, &user, duration)
                        .context("Failed to block user")?;
                    info!("Blocked user {} until time {:?}", user, duration);
                } else {
                    info!(
                        "{} attempted to block {} until {:?} with insufficient privilege {:?}",
                        msg.sender_name, user, duration, msg.privilege
                    );
                }
            }
            Unblock(user) => {
                if msg.privilege >= Privilege::Moderator {
                    database::unblock_user(db_conn, &user).context("Failed to unblock user")?;
                    info!("Unblocked user {}", user);
                } else {
                    info!(
                        "{} attempted to unblock {} with insufficient privilege {:?}",
                        msg.sender_name, user, msg.privilege
                    );
                }
            }
        }
    }

    Ok(())
}
