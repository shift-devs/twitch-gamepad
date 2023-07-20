use std::future::Future;
use std::sync::Arc;

use crate::command::{Movement, MovementPacket};
use tokio::{
    select,
    sync::mpsc::{Receiver, Sender},
};
use uinput::event::{absolute, controller};

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

async fn gamepad_movement<G: Gamepad>(
    gamepad: Arc<tokio::sync::Mutex<&mut G>>,
    packet: MovementPacket,
) -> anyhow::Result<()> {
    let MovementPacket {
        movements,
        duration,
        stagger,
        ..
    } = packet;

    for movement in movements.iter() {
        gamepad.lock().await.press(*movement)?;

        if stagger != 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(stagger)).await;
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(duration)).await;

    for movement in movements.iter().rev() {
        gamepad.lock().await.release(*movement)?;

        if stagger != 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(stagger)).await;
        }
    }

    Ok(())
}

pub async fn gamepad_runner<'a, G: Gamepad + Send + Sync + 'a>(
    gamepad: &'a mut G,
    mut rx: Receiver<MovementPacket>,
) -> anyhow::Result<()> {
    let gamepad = Arc::new(tokio::sync::Mutex::new(gamepad));
    let mut current_packet: Option<MovementPacket> = None;
    let mut current_executor: std::pin::Pin<
        Box<dyn Future<Output = anyhow::Result<()>> + Send + Sync>,
    > = Box::pin(std::future::pending());

    loop {
        select! {
            result = &mut current_executor => {
                result?;
                current_packet = None;
                current_executor = Box::pin(std::future::pending());
            },
            packet = rx.recv() => {
                match packet {
                    Some(packet) => {
                        if packet.interruptible {
                            if let Some(interrupted) = current_packet {
                                std::mem::drop(current_executor);
                                for movement in interrupted.movements.iter().rev() {
                                    gamepad.lock().await.release(*movement)?;
                                }
                            }

                            current_packet = Some(packet.clone());
                            current_executor = Box::pin(gamepad_movement(gamepad.clone(), packet));
                        } else {
                            // Finish any interruptible movement first
                            if current_packet.is_some() {
                                current_executor.await?;
                                current_executor = Box::pin(std::future::pending());
                                current_packet = None;
                            }

                            gamepad_movement(gamepad.clone(), packet).await?;
                        }
                    },
                    None => {
                        if current_packet.is_some() {
                            current_executor.await?;
                        }

                        break Ok(());
                    }
                }
            },
        }
    }
}

pub fn run_gamepad<G: Gamepad + Send + Sync + 'static>(
    mut gamepad: G,
) -> (
    tokio::task::JoinHandle<anyhow::Result<G>>,
    Sender<MovementPacket>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let jh = tokio::task::spawn(async move {
        gamepad_runner(&mut gamepad, rx).await?;
        tracing::info!("Gamepad runner done");
        Ok(gamepad)
    });

    (jh, tx)
}
