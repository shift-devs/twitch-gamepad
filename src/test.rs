use std::collections::BTreeMap;

use tokio::sync::mpsc::Sender;

use crate::{
    command::{self, AnarchyType, Command, Message, Movement, MovementPacket, Privilege},
    config::{Config, GameCommandString, GameInfo, GameName},
    database,
    game_runner::{GameRunner, SfxRequest},
    gamepad::Gamepad,
};

#[derive(Eq, PartialEq, Debug)]
enum ActionType {
    Press,
    Release,
}

#[derive(Default, Debug)]
struct DummyGamepad {
    actions: std::collections::LinkedList<(crate::command::Movement, ActionType)>,
}

impl Gamepad for DummyGamepad {
    fn press(&mut self, movement: crate::command::Movement) -> anyhow::Result<()> {
        self.actions.push_back((movement, ActionType::Press));
        Ok(())
    }

    fn release(&mut self, movement: crate::command::Movement) -> anyhow::Result<()> {
        self.actions.push_back((movement, ActionType::Release));
        Ok(())
    }
}

impl DummyGamepad {
    fn expect_sequence(&self, seq: &[(crate::command::Movement, ActionType)]) {
        eprintln!("expected: {:?}", seq);
        eprintln!("actual: {:?}", self.actions);

        assert_eq!(seq.len(), self.actions.len());
        for (actual, expected) in self.actions.iter().zip(seq.iter()) {
            let (actual_movement, actual_type) = actual;
            let (expected_movement, expected_type) = expected;
            assert_eq!(actual_movement, expected_movement);
            assert_eq!(actual_type, expected_type);
        }
    }
}

#[derive(Debug)]
struct TestSetup {
    msg_rx: tokio::sync::mpsc::Receiver<command::WithReply<Message, Option<String>>>,
    db_conn: rusqlite::Connection,
    gamepad: DummyGamepad,
    game_runner_cmds: Vec<GameRunner>,
    sfx_cmds: Vec<SfxRequest>,
}

impl TestSetup {
    fn new() -> (
        Self,
        tokio::sync::mpsc::Sender<command::WithReply<Message, Option<String>>>,
    ) {
        let db_conn = database::in_memory().unwrap();
        database::clear_db(&db_conn).unwrap();

        let gamepad = DummyGamepad::default();
        let (tx, rx) = tokio::sync::mpsc::channel(10);

        (
            TestSetup {
                msg_rx: rx,
                db_conn,
                gamepad,
                game_runner_cmds: vec![],
                sfx_cmds: vec![],
            },
            tx,
        )
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        self.run_with_games(None).await
    }

    async fn run_with_games(
        &mut self,
        games: Option<BTreeMap<GameName, GameInfo>>,
    ) -> anyhow::Result<()> {
        let config = Config {
            twitch: crate::config::TwitchConfig {
                channel_name: String::new(),
                auth: crate::config::TwitchAuth::Anonymous,
            },
            sound_effects: None,
            games,
        };

        let (mut game_runner_tx, mut rx) = tokio::sync::mpsc::channel(10);
        let game_runner_jh = tokio::task::spawn(async move {
            let mut runner_cmds = Vec::new();
            while let Some(cmd) = rx.recv().await {
                runner_cmds.push(cmd);
            }

            runner_cmds
        });

        let (mut sfx_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sfx_runner_jh = tokio::task::spawn(async move {
            let mut sfx_cmds = Vec::new();
            while let Some(cmd) = rx.recv().await {
                sfx_cmds.push(cmd);
            }

            sfx_cmds
        });

        let gamepad = DummyGamepad::default();
        let (gamepad_jh, gamepad_tx) = crate::gamepad::run_gamepad(gamepad);

        command::run_commands(
            &mut self.msg_rx,
            &config,
            gamepad_tx,
            &mut self.db_conn,
            &mut game_runner_tx,
            Some(&mut sfx_tx),
        )
        .await
        .unwrap();

        let gamepad = gamepad_jh.await.unwrap();
        self.gamepad = gamepad.unwrap();
        std::mem::drop(game_runner_tx);
        std::mem::drop(sfx_tx);

        let mut runner_cmds = game_runner_jh.await.unwrap();
        self.game_runner_cmds.append(&mut runner_cmds);

        let mut sfx_cmds = sfx_runner_jh.await.unwrap();
        self.sfx_cmds.append(&mut sfx_cmds);
        Ok(())
    }
}

async fn send_message(
    tx: &mut Sender<command::WithReply<Message, Option<String>>>,
    msg: Message,
) -> Option<String> {
    let (msg, rx) = command::WithReply::new(msg);
    tx.send(msg).await.unwrap();
    rx.await.unwrap()
}

fn single_movement(movement: Movement) -> Command {
    let movements = vec![movement];
    Command::Movement(MovementPacket {
        movements,
        duration: 50,
        stagger: 0,

        // Don't allow interruption so tests are deterministic
        blocking: true,
    })
}

#[tokio::test]
async fn can_send_multiple_movements() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        let movements = vec![Movement::A, Movement::B];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: true,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn broadcaster_can_send_movements() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn moderator_can_send_movements() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn operator_can_send_movements() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Operator,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_can_send_movements() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_is_subject_to_cooldown() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetCooldown(chrono::Duration::minutes(10)),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn operator_is_not_subject_to_cooldown() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetCooldown(chrono::Duration::minutes(10)),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Operator,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Operator,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_cannot_set_cooldown() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetCooldown(chrono::Duration::minutes(10)),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();

    let cooldown: String = database::get_kv(&test.db_conn, "cooldown")
        .unwrap()
        .unwrap();
    let cooldown = str::parse(&cooldown).unwrap();
    let cooldown = chrono::Duration::milliseconds(cooldown);
    assert!(cooldown.is_zero());
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn user_cannot_set_anarchy_mode() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetAnarchyMode(command::AnarchyType::Anarchy),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();

    let anarchy_mode: String = database::get_kv(&test.db_conn, "anarchy_mode")
        .unwrap()
        .unwrap();
    assert_eq!(&anarchy_mode, command::AnarchyType::Democracy.to_str());
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn anarchy_mode_and_cooldown_restored_from_db() {
    let (mut test, tx) = TestSetup::new();
    database::set_kv(
        &test.db_conn,
        "anarchy_mode",
        AnarchyType::Anarchy.to_str().to_owned(),
    )
    .unwrap();
    database::set_kv(&test.db_conn, "cooldown", "10000").unwrap();

    let join_handle = tokio::task::spawn(async move {
        std::mem::drop(tx);
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();

    let anarchy_mode: String = database::get_kv(&test.db_conn, "anarchy_mode")
        .unwrap()
        .unwrap();
    assert_eq!(&anarchy_mode, command::AnarchyType::Anarchy.to_str());

    let cooldown: String = database::get_kv(&test.db_conn, "cooldown")
        .unwrap()
        .unwrap();
    let cooldown: u64 = str::parse(&cooldown).unwrap();
    assert_eq!(cooldown, 10000);

    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn can_recover_from_malformed_cooldown_or_anarchy_mode_in_db() {
    let (mut test, tx) = TestSetup::new();
    database::set_kv(&test.db_conn, "anarchy_mode", "invalid").unwrap();
    database::set_kv(&test.db_conn, "cooldown", "invalid").unwrap();

    let join_handle = tokio::task::spawn(async move {
        std::mem::drop(tx);
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();

    let anarchy_mode: String = database::get_kv(&test.db_conn, "anarchy_mode")
        .unwrap()
        .unwrap();
    assert_eq!(&anarchy_mode, command::AnarchyType::Democracy.to_str());

    let cooldown: String = database::get_kv(&test.db_conn, "cooldown")
        .unwrap()
        .unwrap();
    let cooldown: u64 = str::parse(&cooldown).unwrap();
    assert_eq!(cooldown, 0);

    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn blocks_and_cooldown_is_ignored_in_anarchy_mode() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::block_user(&mut test.db_conn, &user_name, None).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetCooldown(chrono::Duration::minutes(10)),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: Command::SetAnarchyMode(command::AnarchyType::Anarchy),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
    ]);
}

#[tokio::test]
async fn broadcaster_can_block_user_is_blocked() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Block(user_name.clone(), None),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(database::is_blocked(&mut test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn moderator_can_block_user_is_blocked() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let mod_id = "mod_id".to_owned();
    let mod_name = "mod_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Block(user_name.clone(), None),
                sender_id: mod_id,
                sender_name: mod_name,
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(database::is_blocked(&mut test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_cannot_block_user_is_not_blocked() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let u2_id = "u2_id".to_owned();
    let u2_name = "u2_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Block(user_name.clone(), None),
                sender_id: u2_id,
                sender_name: u2_name,
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(!database::is_blocked(&mut test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
    ]);
}

#[tokio::test]
async fn broadcaster_can_op_user() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::AddOperator(user_name.clone()),
                sender_id: broadcaster_id,
                sender_name: broadcaster_name,
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(database::is_operator(&test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn moderator_can_op_user() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let mod_id = "mod_id".to_owned();
    let mod_name = "mod_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::AddOperator(user_name.clone()),
                sender_id: mod_id,
                sender_name: mod_name,
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(database::is_operator(&test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn operator_cannot_op_user() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let op_id = "operator_id".to_owned();
    let op_name = "operator_name".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::AddOperator(user_name.clone()),
                sender_id: op_id,
                sender_name: op_name,
                privilege: Privilege::Operator,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(!database::is_operator(&test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_can_be_unblocked() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::block_user(&mut test.db_conn, &user_name, None).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Unblock(user_name.clone()),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::B),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(!database::is_blocked(&mut test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_can_be_deoped() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::op_user(&mut test.db_conn, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::RemoveOperator(user_name.clone()),
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(!database::is_operator(&test.db_conn, "user_id").unwrap());
}

#[tokio::test]
async fn user_is_unblocked_after_duration_lapses() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::block_user(&mut test.db_conn, &user_name, Some(chrono::Utc::now())).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: single_movement(command::Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    assert!(!database::is_blocked(&mut test.db_conn, "user_id").unwrap());
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn can_list_blocked_users() {
    let (mut test, mut tx) = TestSetup::new();
    let u1_name = "u1_name".to_owned();
    let u1_id = "u1_id".to_owned();
    let u2_name = "u2_name".to_owned();
    let u2_id = "u2_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    database::update_user(&test.db_conn, &u1_id, &u1_name).unwrap();
    database::block_user(&mut test.db_conn, &u1_name, None).unwrap();

    database::update_user(&test.db_conn, &u2_id, &u2_name).unwrap();
    database::block_user(&mut test.db_conn, &u2_name, None).unwrap();

    let join_handle = tokio::task::spawn(async move {
        let response = send_message(
            &mut tx,
            Message {
                command: Command::ListBlocked,
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await
        .unwrap();

        let blocked_users: Vec<&str> = response.split(", ").collect();
        assert_eq!(blocked_users.len(), 2);
        assert!(blocked_users[0] == u1_name || blocked_users[1] == u1_name);
        assert!(blocked_users[0] == u2_name || blocked_users[1] == u2_name);
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
}

#[tokio::test]
async fn can_list_op_users() {
    let (mut test, mut tx) = TestSetup::new();
    let u1_name = "u1_name".to_owned();
    let u1_id = "u1_id".to_owned();
    let u2_name = "u2_name".to_owned();
    let u2_id = "u2_id".to_owned();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    database::update_user(&test.db_conn, &u1_id, &u1_name).unwrap();
    database::op_user(&mut test.db_conn, &u1_name).unwrap();

    database::update_user(&test.db_conn, &u2_id, &u2_name).unwrap();
    database::op_user(&mut test.db_conn, &u2_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        let response = send_message(
            &mut tx,
            Message {
                command: Command::ListOperators,
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await
        .unwrap();

        let op_users: Vec<&str> = response.split(", ").collect();
        assert_eq!(op_users.len(), 2);
        assert!(op_users[0] == u1_name || op_users[1] == u1_name);
        assert!(op_users[0] == u2_name || op_users[1] == u2_name);
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
}

#[tokio::test]
async fn can_list_games() {
    let (mut test, mut tx) = TestSetup::new();
    let broadcaster_id = "broadcaster_id".to_owned();
    let broadcaster_name = "broadcaster_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 1".to_owned();
    games.insert(
        name,
        GameInfo {
            command: GameCommandString("cmdforgame1 --command".to_owned()),
            restricted_inputs: None,
            controls: None,
        },
    );

    let name: GameName = "Game 2".to_owned();
    games.insert(
        name,
        GameInfo {
            command: GameCommandString("cmdforgame2 --command".to_owned()),
            restricted_inputs: None,
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        let response = send_message(
            &mut tx,
            Message {
                command: Command::ListGames,
                sender_id: broadcaster_id.clone(),
                sender_name: broadcaster_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await
        .unwrap();

        let games: Vec<&str> = response.split(", ").collect();
        assert_eq!(games.len(), 2);
        assert!(games[0] == "Game 1" || games[1] == "Game 1");
        assert!(games[0] == "Game 2" || games[1] == "Game 2");
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();
}

#[tokio::test]
async fn operator_can_save_state() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::op_user(&mut test.db_conn, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SaveState,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::Mode, ActionType::Press),
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::Mode, ActionType::Release),
    ]);
}

#[tokio::test]
async fn operator_can_load_state() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::op_user(&mut test.db_conn, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::LoadState,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::Mode, ActionType::Press),
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
        (Movement::Mode, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_cannot_save_state() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SaveState,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn user_cannot_load_state() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::LoadState,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn operator_can_reset_game() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();
    database::op_user(&mut test.db_conn, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Reset,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::Mode, ActionType::Press),
        (Movement::X, ActionType::Press),
        (Movement::X, ActionType::Release),
        (Movement::Mode, ActionType::Release),
    ]);
}

#[tokio::test]
async fn user_cannot_reset_game() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    database::update_user(&test.db_conn, &user_id, &user_name).unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Reset,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn moderator_can_switch_games() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 1".to_owned();
    games.insert(
        name,
        GameInfo {
            command: GameCommandString("cmdforgame1 --command".to_owned()),
            restricted_inputs: None,
            controls: None,
        },
    );

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd.clone(),
            restricted_inputs: None,
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(
        test.game_runner_cmds[0],
        GameRunner::SwitchTo(game2_cmd.to_command())
    );
}

#[tokio::test]
async fn moderator_can_stop_gameplay() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 1".to_owned();
    games.insert(
        name,
        GameInfo {
            command: GameCommandString("cmdforgame1 --command".to_owned()),
            restricted_inputs: None,
            controls: None,
        },
    );

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd,
            restricted_inputs: None,
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Stop,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(test.game_runner_cmds[0], GameRunner::Stop);
}

#[tokio::test]
async fn user_cannot_switch_games() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 1".to_owned();
    games.insert(
        name,
        GameInfo {
            command: GameCommandString("cmdforgame1 --command".to_owned()),
            restricted_inputs: None,
            controls: None,
        },
    );

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd,
            restricted_inputs: None,
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 0);
}

#[tokio::test]
async fn user_cannot_stop_gameplay() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 1".to_owned();
    games.insert(
        name,
        GameInfo {
            command: GameCommandString("cmdforgame1 --command".to_owned()),
            restricted_inputs: None,
            controls: None,
        },
    );

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd,
            restricted_inputs: None,
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Stop,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 0);
}

#[tokio::test]
async fn restricted_inputs_are_blocked_in_normal_modes() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd.clone(),
            restricted_inputs: Some(vec!["start".to_owned()]),
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        let movements = vec![Movement::Start, Movement::B];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: true,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(
        test.game_runner_cmds[0],
        GameRunner::SwitchTo(game2_cmd.to_command())
    );
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn restricted_inputs_are_not_blocked_in_restricted_mode() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let op_id = "op_id".to_owned();
    let op_name = "op_name".to_owned();

    database::update_user(&test.db_conn, &op_id, &op_name).unwrap();
    database::op_user(&mut test.db_conn, &op_name).unwrap();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd.clone(),
            restricted_inputs: Some(vec!["start".to_owned()]),
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::SetAnarchyMode(AnarchyType::Restricted),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        let movements = vec![Movement::Start, Movement::B];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: true,
                }),
                sender_id: op_id.clone(),
                sender_name: op_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(
        test.game_runner_cmds[0],
        GameRunner::SwitchTo(game2_cmd.to_command())
    );
    test.gamepad.expect_sequence(&[
        (Movement::Start, ActionType::Press),
        (Movement::B, ActionType::Press),
        (Movement::B, ActionType::Release),
        (Movement::Start, ActionType::Release),
    ]);
}

#[tokio::test]
async fn users_cannot_send_input_in_restricted_mode() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mod_id = "mod_id".to_owned();
    let mod_name = "mod_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd.clone(),
            restricted_inputs: Some(vec!["start".to_owned()]),
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: mod_id.clone(),
                sender_name: mod_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::SetAnarchyMode(AnarchyType::Restricted),
                sender_id: mod_id.clone(),
                sender_name: mod_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: single_movement(Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Standard,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(
        test.game_runner_cmds[0],
        GameRunner::SwitchTo(game2_cmd.to_command())
    );
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn can_interrupt_movements_with_direction() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        let movements = vec![Movement::Left];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,

                    // Set a duration >= 1 minute
                    // We shouldn't execute the whole thing
                    duration: 1000 * 60 * 2,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        // Make sure the above movement is able to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let movements = vec![Movement::Start];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    let timeout = tokio::time::timeout(tokio::time::Duration::from_secs(2), test.run());
    timeout.await.unwrap().unwrap();

    join_handle.await.unwrap();

    test.gamepad.expect_sequence(&[
        (Movement::Left, ActionType::Press),
        (Movement::Left, ActionType::Release),
        (Movement::Start, ActionType::Press),
        (Movement::Start, ActionType::Release),
    ]);
}

#[tokio::test]
async fn only_directional_movements_are_interrupted() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        let movements = vec![Movement::Left, Movement::B];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,

                    // Set a duration >= 1 minute
                    // We shouldn't execute the whole thing
                    duration: 400,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        // Make sure the above movement is able to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let movements = vec![Movement::Start];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    let timeout = tokio::time::timeout(tokio::time::Duration::from_secs(2), test.run());
    timeout.await.unwrap().unwrap();

    join_handle.await.unwrap();

    test.gamepad.expect_sequence(&[
        (Movement::Left, ActionType::Press),
        (Movement::B, ActionType::Press),
        (Movement::Left, ActionType::Release),
        (Movement::Start, ActionType::Press),
        (Movement::Start, ActionType::Release),
        (Movement::B, ActionType::Release),
    ]);
}

#[tokio::test]
async fn saving_cannot_be_interrupted() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        let movements = vec![Movement::Select];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 250,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::SaveState,
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        let movements = vec![Movement::Start];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    test.run().await.unwrap();
    join_handle.await.unwrap();
    test.gamepad.expect_sequence(&[
        (Movement::Select, ActionType::Press),
        (Movement::Select, ActionType::Release),
        (Movement::Mode, ActionType::Press),
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::Mode, ActionType::Release),
        (Movement::Start, ActionType::Press),
        (Movement::Start, ActionType::Release),
    ]);
}

#[tokio::test]
async fn same_button_presses_are_sequenced() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        let movements = vec![Movement::A];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements: movements.clone(),
                    duration: 100,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements: movements.clone(),
                    duration: 100,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: false,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Broadcaster,
            },
        )
        .await;
    });

    let timeout = tokio::time::timeout(tokio::time::Duration::from_secs(2), test.run());
    timeout.await.unwrap().unwrap();

    join_handle.await.unwrap();

    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}

#[tokio::test]
async fn sfx_are_enabled_in_stream_mode_and_games_cannot_be_started() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd.clone(),
            restricted_inputs: Some(vec!["start".to_owned()]),
            controls: None,
        },
    );

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetAnarchyMode(AnarchyType::Streaming),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        let movements = vec![Movement::Start, Movement::B];
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(MovementPacket {
                    movements,
                    duration: 50,
                    stagger: 0,
                    blocking: true,
                }),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    dbg!(&test);

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(test.game_runner_cmds[0], GameRunner::Stop);
    assert_eq!(test.sfx_cmds.len(), 2);
    assert_eq!(test.sfx_cmds[0], SfxRequest::Enable(false));
    assert_eq!(test.sfx_cmds[1], SfxRequest::Enable(true));
    test.gamepad.expect_sequence(&[]);
}

#[tokio::test]
async fn games_can_be_started_after_switching_from_stream_mode() {
    let (mut test, mut tx) = TestSetup::new();
    let user_id = "user_id".to_owned();
    let user_name = "user_name".to_owned();

    let mut games: BTreeMap<GameName, GameInfo> = BTreeMap::new();

    let name: GameName = "Game 2".to_owned();
    let game2_cmd = GameCommandString("cmdforgame2 --command".to_owned());
    games.insert(
        name,
        GameInfo {
            command: game2_cmd.clone(),
            restricted_inputs: Some(vec!["start".to_owned()]),
            controls: None,
        },
    );

    // Initialize in streaming setting
    database::set_kv(&test.db_conn, "anarchy_mode", "streaming").unwrap();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::SetAnarchyMode(AnarchyType::Democracy),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: Command::Game("Game 2".to_owned()),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;

        send_message(
            &mut tx,
            Message {
                command: single_movement(Movement::A),
                sender_id: user_id.clone(),
                sender_name: user_name.clone(),
                privilege: Privilege::Moderator,
            },
        )
        .await;
    });

    test.run_with_games(Some(games)).await.unwrap();
    join_handle.await.unwrap();

    dbg!(&test);

    assert_eq!(test.game_runner_cmds.len(), 1);
    assert_eq!(
        test.game_runner_cmds[0],
        GameRunner::SwitchTo(game2_cmd.to_command())
    );
    assert_eq!(test.sfx_cmds.len(), 1);
    assert_eq!(test.sfx_cmds[0], SfxRequest::Enable(false));
    test.gamepad.expect_sequence(&[
        (Movement::A, ActionType::Press),
        (Movement::A, ActionType::Release),
    ]);
}
