use crate::{
    config::{Config, GameName},
    database,
    game_runner::{self, GameRunner},
};
use anyhow::{anyhow, Context};

use rusqlite::Connection;
use strum_macros::EnumIter;
use tokio::sync::{
    mpsc::{Receiver, Sender},
    oneshot,
};
use tracing::info;

const CONFIG_KV_ANARCHY_MODE: &str = "anarchy_mode";
const CONFIG_KV_COOLDOWN_DURATION: &str = "cooldown";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AnarchyType {
    Anarchy,
    Democracy,
}

impl AnarchyType {
    pub const fn to_str(self) -> &'static str {
        match self {
            Self::Anarchy => "anarchy",
            Self::Democracy => "democracy",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "anarchy" => Some(Self::Anarchy),
            "democracy" => Some(Self::Democracy),
            _ => None,
        }
    }
}

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

#[derive(Copy, Clone, Debug, Eq, PartialEq, EnumIter)]
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
    Mode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovementPacket {
    pub movements: Vec<Movement>,
    pub duration: u64,
    pub stagger: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PartialCommand {
    AddOperator,
    RemoveOperator,
    Block,
    Unblock,
    Game,
    List,
    SetCooldown,
    SetAnarchyMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    Movement(MovementPacket),
    AddOperator(String),
    RemoveOperator(String),
    Block(String, Option<chrono::DateTime<chrono::Utc>>),
    Unblock(String),
    Game(GameName),
    Stop,
    Partial(PartialCommand),
    ListBlocked,
    ListOperators,
    ListGames,
    PrintHelp,
    SaveState,
    LoadState,
    Reset,
    SetCooldown(chrono::Duration),
    SetAnarchyMode(AnarchyType),
    PrintAnarchyMode,
}

fn parse_movement(tokens: &Vec<&str>) -> Option<Command> {
    fn parse_movement_token(token: &str) -> Option<Movement> {
        let movement = match token {
            "a" => Movement::A,
            "b" => Movement::B,
            "c" => Movement::C,
            "x" => Movement::X,
            "y" => Movement::Y,
            "z" => Movement::Z,
            "tl" | "lt" => Movement::TL,
            "tr" | "rt" => Movement::TR,
            "up" => Movement::Up,
            "down" => Movement::Down,
            "left" => Movement::Left,
            "right" => Movement::Right,
            "start" => Movement::Start,
            "select" => Movement::Select,
            //"mode" => Movement::Mode,
            _ => return None,
        };

        Some(movement)
    }

    if tokens.is_empty() {
        return None;
    }

    let mut movements = Vec::new();
    let mut duration = Some(500);
    for (idx, token) in tokens.iter().enumerate() {
        if let Some(movement) = parse_movement_token(token) {
            movements.push(movement);
        } else if idx == tokens.len() - 1 {
            duration = str::parse::<u64>(token)
                .ok()
                .filter(|sec| *sec <= 5)
                .map(|sec| sec * 1000);
        } else {
            return None;
        }
    }

    if movements.is_empty() {
        return None;
    }

    duration.map(|duration| {
        Command::Movement(MovementPacket {
            movements,
            duration,
            stagger: 0,
        })
    })
}

pub fn parse_command(input: &str) -> Option<Command> {
    let mut tokens: Vec<String> = input.split_whitespace().map(|t| t.to_lowercase()).collect();
    tokens.retain(|token| *token != "\u{e0000}");

    let tokens: Vec<&str> = tokens.iter().map(|t| t.as_str()).collect();
    if let Some(cmd) = parse_movement(&tokens) {
        return Some(cmd);
    }

    match &tokens[..] {
        ["tp", "block"] => Some(Command::Partial(PartialCommand::Block)),
        ["tp", "block", target] => Some(Command::Block(target.to_string(), None)),
        ["tp", "block", target, duration] => duration_str::parse(duration)
            .ok()
            .and_then(|d| chrono::Duration::from_std(d).ok())
            .map(|d| chrono::Utc::now() + d)
            .map(|d| Command::Block(target.to_string(), Some(d)))
            .or(Some(Command::Partial(PartialCommand::Block))),
        ["tp", "unblock"] => Some(Command::Partial(PartialCommand::Unblock)),
        ["tp", "unblock", target] => Some(Command::Unblock(target.to_string())),
        ["tp", "op"] => Some(Command::Partial(PartialCommand::AddOperator)),
        ["tp", "op", target] => Some(Command::AddOperator(target.to_string())),
        ["tp", "deop"] => Some(Command::Partial(PartialCommand::RemoveOperator)),
        ["tp", "deop", target] => Some(Command::RemoveOperator(target.to_string())),
        ["tp", "games"] => Some(Command::ListGames),
        ["tp", "game" | "switch" | "start"] => Some(Command::Partial(PartialCommand::Game)),
        ["tp", "game" | "switch" | "start", game] => {
            let game: GameName = game.to_string();
            Some(Command::Game(game))
        }
        ["tp", "stop"] => Some(Command::Stop),
        ["tp", "list"] => Some(Command::Partial(PartialCommand::List)),
        ["tp", "list", "games"] => Some(Command::ListGames),
        ["tp", "help" | "commands"] => Some(Command::PrintHelp),
        ["tp", "list", "block" | "blocks" | "blocked"] => Some(Command::ListBlocked),
        ["tp", "list", "ops" | "operators" | "op"] => Some(Command::ListOperators),
        ["tp", "save"] => Some(Command::SaveState),
        ["tp", "load"] => Some(Command::LoadState),
        ["tp", "reset"] => Some(Command::Reset),
        ["tp", "mode"] => Some(Command::PrintAnarchyMode),
        ["tp", "mode", "anarchy"] => Some(Command::SetAnarchyMode(AnarchyType::Anarchy)),
        ["tp", "mode", "democracy"] => Some(Command::SetAnarchyMode(AnarchyType::Democracy)),
        ["tp", "mode", _] => Some(Command::Partial(PartialCommand::SetAnarchyMode)),
        ["tp", "cooldown"] => Some(Command::Partial(PartialCommand::SetCooldown)),
        ["tp", "cooldown", cd] => duration_str::parse(cd)
            .ok()
            .and_then(|d| chrono::Duration::from_std(d).ok())
            .map(Command::SetCooldown)
            .or(Some(Command::Partial(PartialCommand::SetCooldown))),
        _ => None,
    }
}

pub async fn run_commands(
    rx: &mut Receiver<WithReply<Message, Option<String>>>,
    config: &Config,
    gamepad_tx: Sender<MovementPacket>,
    db_conn: &mut Connection,
    game_runner_tx: &mut Sender<game_runner::GameRunner>,
) -> anyhow::Result<()> {
    let game_commands = config.game_command_list();

    let anarchy_mode = database::get_or_set_kv(
        db_conn,
        CONFIG_KV_ANARCHY_MODE,
        AnarchyType::Democracy.to_str().to_owned(),
    )?;
    let mut anarchy_mode = match AnarchyType::from_str(&anarchy_mode) {
        Some(am) => am,
        None => {
            tracing::warn!(
                "Invalid anarchy_mode {} in database, defaulting to democracy",
                anarchy_mode
            );
            database::set_kv(
                db_conn,
                CONFIG_KV_ANARCHY_MODE,
                AnarchyType::Democracy.to_str(),
            )?;
            AnarchyType::Democracy
        }
    };

    let cooldown: String =
        database::get_or_set_kv(db_conn, CONFIG_KV_COOLDOWN_DURATION, "0".to_owned())?;
    let cooldown = match str::parse(&cooldown) {
        Ok(cd) => cd,
        Err(_) => {
            tracing::warn!("Invalid cooldown {} in database, defaulting to 0", cooldown);
            database::set_kv(db_conn, CONFIG_KV_COOLDOWN_DURATION, "0")?;
            0
        }
    };

    let mut cooldown = chrono::Duration::milliseconds(cooldown);

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

        if msg.privilege < Privilege::Operator
            && matches!(anarchy_mode, AnarchyType::Democracy)
            && !cooldown.is_zero()
            && !database::test_and_set_cooldown_lapsed(db_conn, &msg.sender_id, &cooldown)?
        {
            reply_tx
                .send(None)
                .map_err(|_| anyhow!("Failed to reply to command"))?;
            continue;
        }

        match msg.command {
            SetAnarchyMode(am) => {
                if msg.privilege >= Privilege::Moderator {
                    anarchy_mode = am;
                    database::set_kv(db_conn, CONFIG_KV_ANARCHY_MODE, anarchy_mode.to_str())?;
                    reply_tx
                        .send(Some(format!("Set mode to {}", anarchy_mode.to_str())))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            PrintAnarchyMode => {
                reply_tx
                    .send(Some(format!("Current mode is {}", anarchy_mode.to_str())))
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            SetCooldown(cd) => {
                if msg.privilege >= Privilege::Moderator {
                    database::set_kv(db_conn, CONFIG_KV_COOLDOWN_DURATION, cd.num_milliseconds())?;
                    cooldown = cd;
                    reply_tx
                        .send(Some(format!("Set cooldown to {}", cooldown)))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            Movement(packet) => {
                reply_tx
                    .send(None)
                    .map_err(|_| anyhow!("Failed to reply to command"))?;

                if matches!(anarchy_mode, AnarchyType::Anarchy)
                    || !database::is_blocked(db_conn, &msg.sender_id)
                        .context("Failed to check for blocked user")?
                {
                    info!("Sending movement {:?}", packet);
                    gamepad_tx.send(packet).await?;
                } else {
                    info!("Blocked movement from {}", msg.sender_name);
                }
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
                    let user_blocked = database::block_user(db_conn, &user, duration)
                        .context("Failed to block user")?;

                    let reply_msg = if user_blocked {
                        info!("Blocked user {} until time {:?}", user, duration);
                        format!(
                            "Blocked {} {}",
                            user,
                            if let Some(duration) = duration {
                                format!("until {}", duration)
                            } else {
                                "forever".to_owned()
                            }
                        )
                    } else {
                        info!("Block for user {} cannot be applied, unknown user", user);
                        format!("Could not find user {}, they probably haven't played", user)
                    };

                    reply_tx
                        .send(Some(reply_msg))
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
            Game(game) => {
                if msg.privilege >= Privilege::Moderator {
                    if let Some(game_command) = game_commands.get(&game) {
                        game_runner_tx
                            .send(GameRunner::SwitchTo(game_command.clone()))
                            .await?;
                        reply_tx
                            .send(None)
                            .map_err(|_| anyhow!("Failed to reply to command"))?;
                    } else {
                        reply_tx
                            .send(Some(format!(
                                "No game {} found, see full list with \"tp games\"",
                                game
                            )))
                            .map_err(|_| anyhow!("Failed to reply to command"))?;
                    }
                } else {
                    info!(
                        "{} attempted to switch game to {} with insufficient privilege {:?}",
                        msg.sender_name, game, msg.privilege
                    );

                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            Stop => {
                if msg.privilege >= Privilege::Moderator {
                    game_runner_tx.send(GameRunner::Stop).await?;
                    reply_tx
                        .send(None)
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to stop with insufficient privilege {:?}",
                        msg.sender_name, msg.privilege
                    );

                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            Partial(partial) => {
                use PartialCommand::*;
                let diag_msg = match partial {
                    AddOperator => "Usage: tp op <user>",
                    RemoveOperator => "Usage: tp deop <user>",
                    Block => "Usage: tp block <user> [optional: duration]",
                    Unblock => "Usage: tp unblock <user>",
                    Game => "Usage: tp game <game-name>",
                    List => "Usage: tp list games | blocked | ops",
                    SetCooldown => "Usage: tp cooldown <duration>",
                    SetAnarchyMode => "Usage: tp mode <anarchy | democracy>",
                };

                reply_tx
                    .send(Some(diag_msg.to_string()))
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            ListGames => {
                let games: Vec<&str> = game_commands.keys().map(|game| game.as_str()).collect();
                reply_tx
                    .send(Some(games.join(", ")))
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            ListOperators => {
                let operators = database::list_op_users(db_conn)?;
                reply_tx
                    .send(Some(operators.join(", ")))
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            ListBlocked => {
                let blocked_users = database::list_blocked_users(db_conn)?;
                reply_tx
                    .send(Some(blocked_users.join(", ")))
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            PrintHelp => {
                let mut available_commands = Vec::new();
                available_commands
                    .push("Move with standard controller buttons (up, down, a, b, tl, tr, etc.)");
                if msg.privilege >= Privilege::Operator {
                    available_commands.push("tp save/load - save or load state");
                }
                if msg.privilege >= Privilege::Moderator {
                    available_commands.push("tp block/unblock - block or unblock a user");
                    available_commands.push("tp op/deop - promote user to operator");
                    available_commands.push("tp list - list games/ops/blocked users");
                    available_commands.push("tp game/reset - switch/reset game");
                    available_commands.push("tp mode - set anarchy mode");
                    available_commands.push("tp cooldown - set command cooldown");
                }
                reply_tx
                    .send(Some(available_commands.join(", ")))
                    .map_err(|_| anyhow!("Failed to reply to command"))?;
            }
            SaveState => {
                if msg.privilege >= Privilege::Operator {
                    use crate::command::Movement;

                    // FIXME: Make this more generic
                    // Right now it's tied to a specific hotkey combo in retroarch
                    let movements = vec![Movement::Mode, Movement::A];
                    gamepad_tx
                        .send(MovementPacket {
                            movements,
                            duration: 100,
                            stagger: 100,
                        })
                        .await?;

                    info!("{} saved state", msg.sender_name);
                    reply_tx
                        .send(Some("Saved game state".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to save state with insufficient privilege {:?}",
                        msg.sender_name, msg.privilege
                    );
                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            LoadState => {
                if msg.privilege >= Privilege::Operator {
                    use crate::command::Movement;

                    // FIXME: Make this more generic
                    // Right now it's tied to a specific hotkey combo in retroarch
                    let movements = vec![Movement::Mode, Movement::B];
                    gamepad_tx
                        .send(MovementPacket {
                            movements,
                            duration: 100,
                            stagger: 100,
                        })
                        .await?;

                    info!("{} loaded state", msg.sender_name);
                    reply_tx
                        .send(Some("Loaded game state".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to save state with insufficient privilege {:?}",
                        msg.sender_name, msg.privilege
                    );
                    reply_tx
                        .send(Some("You don't have permission to do that".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                }
            }
            Reset => {
                if msg.privilege >= Privilege::Operator {
                    use crate::command::Movement;

                    // FIXME: Make this more generic
                    // Right now it's tied to a specific hotkey combo in retroarch
                    let movements = vec![Movement::Mode, Movement::X];
                    gamepad_tx
                        .send(MovementPacket {
                            movements,
                            duration: 100,
                            stagger: 100,
                        })
                        .await?;

                    info!("{} reset the system", msg.sender_name);
                    reply_tx
                        .send(Some("Reset current game".to_string()))
                        .map_err(|_| anyhow!("Failed to reply to command"))?;
                } else {
                    info!(
                        "{} attempted to save state with insufficient privilege {:?}",
                        msg.sender_name, msg.privilege
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
    use super::{parse_command, Command, Movement, PartialCommand};

    macro_rules! test_command {
        ($id: ident, $cmd: expr, $result: expr) => {
            #[test]
            fn $id() {
                assert_eq!(parse_command($cmd), $result);
            }
        };
    }

    fn movement_packet(movements: &[Movement], duration: u64) -> Option<Command> {
        let movements = Vec::from(movements);
        Some(Command::Movement(super::MovementPacket {
            movements,
            duration,
            stagger: 0,
        }))
    }

    test_command!(
        parse_movement_case_sensitivity,
        "A",
        movement_packet(&[Movement::A], 500)
    );
    test_command!(parse_movement_a, "a", movement_packet(&[Movement::A], 500));
    test_command!(parse_movement_b, "b", movement_packet(&[Movement::B], 500));
    test_command!(parse_movement_c, "c", movement_packet(&[Movement::C], 500));
    test_command!(parse_movement_x, "x", movement_packet(&[Movement::X], 500));
    test_command!(parse_movement_y, "y", movement_packet(&[Movement::Y], 500));
    test_command!(parse_movement_z, "z", movement_packet(&[Movement::Z], 500));
    test_command!(
        parse_movement_tl,
        "tl",
        movement_packet(&[Movement::TL], 500)
    );
    test_command!(
        parse_movement_tr,
        "tr",
        movement_packet(&[Movement::TR], 500)
    );
    test_command!(
        parse_movement_start,
        "start",
        movement_packet(&[Movement::Start], 500)
    );
    test_command!(
        parse_movement_select,
        "select",
        movement_packet(&[Movement::Select], 500)
    );
    test_command!(
        parse_movement_up,
        "up",
        movement_packet(&[Movement::Up], 500)
    );
    test_command!(
        parse_movement_down,
        "down",
        movement_packet(&[Movement::Down], 500)
    );
    test_command!(
        parse_movement_left,
        "left",
        movement_packet(&[Movement::Left], 500)
    );
    test_command!(
        parse_movement_right,
        "right",
        movement_packet(&[Movement::Right], 500)
    );
    test_command!(
        parse_movement_duration,
        "a 2",
        movement_packet(&[Movement::A], 2000)
    );
    test_command!(
        parse_movement_multiple_with_time,
        "a b x y lt rt 1",
        movement_packet(
            &[
                Movement::A,
                Movement::B,
                Movement::X,
                Movement::Y,
                Movement::TL,
                Movement::TR
            ],
            1000
        )
    );
    test_command!(
        parse_movement_multiple,
        "a b x y lt rt",
        movement_packet(
            &[
                Movement::A,
                Movement::B,
                Movement::X,
                Movement::Y,
                Movement::TL,
                Movement::TR
            ],
            500
        )
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

    test_command!(parse_help_help, "tp help", Some(Command::PrintHelp));
    test_command!(parse_help_commands, "tp commands", Some(Command::PrintHelp));
    test_command!(
        parse_list_blocked_blocked,
        "tp list blocked",
        Some(Command::ListBlocked)
    );
    test_command!(
        parse_list_blocked_blocks,
        "tp list blocks",
        Some(Command::ListBlocked)
    );
    test_command!(
        parse_list_blocked_block,
        "tp list block",
        Some(Command::ListBlocked)
    );
    test_command!(
        parse_list_op_ops,
        "tp list ops",
        Some(Command::ListOperators)
    );
    test_command!(
        parse_list_op_operators,
        "tp list operators",
        Some(Command::ListOperators)
    );
    test_command!(parse_list_op_op, "tp list op", Some(Command::ListOperators));
    test_command!(parse_list_games, "tp list games", Some(Command::ListGames));
    test_command!(
        parse_list_games_direct,
        "tp games",
        Some(Command::ListGames)
    );

    test_command!(parse_invalid, "asdf", None);
    test_command!(
        parse_extraneous,
        "tp block user 3m and then something",
        None
    );
    test_command!(parse_movement_extraneous, "a 2 and something", None);
    test_command!(parse_movement_invalid_time, "a invalid-time", None);
    test_command!(parse_movement_time_too_large, "a 100", None);
    test_command!(
        parse_twitch_deduplicated,
        "a \u{e0000}",
        movement_packet(&[Movement::A], 500)
    );
    test_command!(
        parse_block_invalid_time,
        "tp block user notatime",
        Some(Command::Partial(PartialCommand::Block))
    );

    test_command!(
        parse_partial_block,
        "tp block",
        Some(Command::Partial(PartialCommand::Block))
    );
    test_command!(
        parse_partial_unblock,
        "tp unblock",
        Some(Command::Partial(PartialCommand::Unblock))
    );
    test_command!(
        parse_partial_op,
        "tp op",
        Some(Command::Partial(PartialCommand::AddOperator))
    );
    test_command!(
        parse_partial_deop,
        "tp deop",
        Some(Command::Partial(PartialCommand::RemoveOperator))
    );
    test_command!(
        parse_partial_list,
        "tp list",
        Some(Command::Partial(PartialCommand::List))
    );
    test_command!(
        parse_malformed_mode,
        "tp mode invalidmode",
        Some(Command::Partial(PartialCommand::SetAnarchyMode))
    );
    test_command!(
        parse_partial_cooldown,
        "tp cooldown",
        Some(Command::Partial(PartialCommand::SetCooldown))
    );

    test_command!(parse_save, "tp save", Some(Command::SaveState));
    test_command!(parse_load, "tp load", Some(Command::LoadState));
    test_command!(parse_reset, "tp reset", Some(Command::Reset));

    test_command!(parse_print_mode, "tp mode", Some(Command::PrintAnarchyMode));
    test_command!(
        parse_anarchy,
        "tp mode anarchy",
        Some(Command::SetAnarchyMode(
            crate::command::AnarchyType::Anarchy
        ))
    );
    test_command!(
        parse_democracy,
        "tp mode democracy",
        Some(Command::SetAnarchyMode(
            crate::command::AnarchyType::Democracy
        ))
    );

    test_command!(
        parse_partial_game,
        "tp game",
        Some(Command::Partial(PartialCommand::Game))
    );
    test_command!(
        parse_game,
        "tp game some_game",
        Some(Command::Game("some_game".to_string()))
    );
    test_command!(parse_stop, "tp stop", Some(Command::Stop));

    test_command!(
        parse_cooldown,
        "tp cooldown 10s",
        Some(Command::SetCooldown(chrono::Duration::seconds(10)))
    );

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
