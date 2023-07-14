use tokio::{sync::mpsc::{Receiver, Sender}, select};

use crate::command::{Movement, MovementPacket};
use uinput::event::{absolute, controller};
use strum::IntoEnumIterator;
use tracing::info;

pub trait Gamepad {
    fn press(&mut self, movement: Movement) -> anyhow::Result<()>;
    fn release(&mut self, movement: Movement) -> anyhow::Result<()>;
}

pub struct UinputGamepad {
    gamepad: uinput::Device,
}

impl UinputGamepad {
    pub fn new() -> anyhow::Result<Self> {
        let mut gamepad = uinput::default()?
            .name("Twitch Gamepad")?
            .event(controller::Controller::All)?
            .event(absolute::Absolute::Position(absolute::Position::X))?
            .min(0)
            .max(255)
            .fuzz(0)
            .flat(0)
            .event(absolute::Absolute::Position(absolute::Position::Y))?
            .min(0)
            .max(255)
            .fuzz(0)
            .flat(0)
            .create()?;

        gamepad.send(absolute::Absolute::Position(absolute::Position::X), 128)?;
        gamepad.send(absolute::Absolute::Position(absolute::Position::Y), 128)?;
        gamepad.synchronize()?;

        Ok(UinputGamepad { gamepad })
    }

    fn map_movement(movement: &Movement) -> controller::Controller {
        use controller::{Controller, DPad, GamePad};
        use Movement::*;
        match movement {
            A => Controller::GamePad(GamePad::A),
            B => Controller::GamePad(GamePad::B),
            C => Controller::GamePad(GamePad::C),
            X => Controller::GamePad(GamePad::X),
            Y => Controller::GamePad(GamePad::Y),
            Z => Controller::GamePad(GamePad::Z),
            TL => Controller::GamePad(GamePad::TL),
            TR => Controller::GamePad(GamePad::TR),
            Up => Controller::DPad(DPad::Up),
            Down => Controller::DPad(DPad::Down),
            Left => Controller::DPad(DPad::Left),
            Right => Controller::DPad(DPad::Right),
            Start => Controller::GamePad(GamePad::Start),
            Select => Controller::GamePad(GamePad::Select),
            Mode => Controller::GamePad(GamePad::Mode),
        }
    }
}

impl Gamepad for UinputGamepad {
    fn press(&mut self, movement: Movement) -> anyhow::Result<()> {
        let cmd = Self::map_movement(&movement);

        self.gamepad.press(&cmd)?;
        self.gamepad.synchronize()?;
        Ok(())
    }

    fn release(&mut self, movement: Movement) -> anyhow::Result<()> {
        let cmd = Self::map_movement(&movement);

        self.gamepad.release(&cmd)?;
        self.gamepad.synchronize()?;
        Ok(())
    }
}

pub async fn gamepad_runner<G: Gamepad>(gamepad: &mut G, mut rx: Receiver<MovementPacket>) -> anyhow::Result<()> {
    while let Some(MovementPacket { movements, duration, stagger }) = rx.recv().await {
        for movement in movements.iter() {
            gamepad.press(*movement)?;

            if stagger != 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(stagger)).await;
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(duration)).await;

        for movement in movements.iter() {
            gamepad.release(*movement)?;

            if stagger != 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(stagger)).await;
            }
        }
    }

    Ok(())
}

pub fn run_gamepad<G: Gamepad + Send + Sync + 'static>(mut gamepad: G) -> (tokio::task::JoinHandle<G>, Sender<MovementPacket>) {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let jh = tokio::task::spawn(async move {
        //let mut gamepad = gamepad;
        gamepad_runner(&mut gamepad, rx).await.unwrap();
        gamepad
    });

    (jh, tx)
}
