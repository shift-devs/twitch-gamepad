use std::collections::BTreeMap;

use tokio::sync::mpsc::Sender;

use crate::{
    command::{self, Command, Message, Movement, Privilege},
    config::{Config, GameCommandString, GameName},
    database,
    gamepad::Gamepad,
};

#[derive(Eq, PartialEq, Debug)]
enum ActionType {
    Press,
    Release,
}

#[derive(Default)]
struct DummyGamepad {
    actions: std::collections::LinkedList<(crate::command::Movement, ActionType)>,
}

impl Gamepad for DummyGamepad {
    fn press(&mut self, movement: &crate::command::Movement) -> anyhow::Result<()> {
        self.actions.push_back((*movement, ActionType::Press));
        Ok(())
    }

    fn release(&mut self, movement: &crate::command::Movement) -> anyhow::Result<()> {
        self.actions.push_back((*movement, ActionType::Release));
        Ok(())
    }
}

impl DummyGamepad {
    fn expect_sequence(&self, seq: &[(crate::command::Movement, ActionType)]) {
        assert_eq!(seq.len(), self.actions.len());
        for (actual, expected) in self.actions.iter().zip(seq.iter()) {
            let (actual_movement, actual_type) = actual;
            let (expected_movement, expected_type) = expected;
            assert_eq!(actual_movement, expected_movement);
            assert_eq!(actual_type, expected_type);
        }
    }
}

struct TestSetup {
    msg_rx: tokio::sync::mpsc::Receiver<command::WithReply<Message, Option<String>>>,
    db_conn: rusqlite::Connection,
    gamepad: DummyGamepad,
}

impl TestSetup {
    fn new() -> (
        Self,
        tokio::sync::mpsc::Sender<command::WithReply<Message, Option<String>>>,
    ) {
        let db_conn = database::in_memory().unwrap();
        database::clear_db(&db_conn).unwrap();
        eprintln!("new called");

        let gamepad = DummyGamepad::default();
        let (tx, rx) = tokio::sync::mpsc::channel(10);

        (
            TestSetup {
                msg_rx: rx,
                db_conn,
                gamepad,
            },
            tx,
        )
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        self.run_with_games(None).await
    }

    async fn run_with_games(
        &mut self,
        games: Option<BTreeMap<GameName, GameCommandString>>,
    ) -> anyhow::Result<()> {
        let config = Config {
            twitch: crate::config::TwitchConfig {
                channel_name: String::new(),
                auth: crate::config::TwitchAuth::Anonymous,
            },
            games,
        };
        command::run_commands(
            &mut self.msg_rx,
            &config,
            &mut self.gamepad,
            &mut self.db_conn,
        )
        .await?;
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

#[tokio::test]
async fn broadcaster_can_send_movements() {
    let (mut test, mut tx) = TestSetup::new();
    let user_name = "user_name".to_owned();
    let user_id = "user_id".to_owned();

    let join_handle = tokio::task::spawn(async move {
        send_message(
            &mut tx,
            Message {
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::B, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::B, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::B, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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
                command: Command::Movement(command::Movement::B, 500),
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
                command: Command::Movement(command::Movement::A, 500),
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

    let mut games: BTreeMap<GameName, GameCommandString> = BTreeMap::new();

    let name: GameName = "Game 1".to_owned();
    games.insert(name, GameCommandString("cmdforgame1 --command".to_owned()));

    let name: GameName = "Game 2".to_owned();
    games.insert(name, GameCommandString("cmdforgame2 --command".to_owned()));

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
