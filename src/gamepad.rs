use std::collections::VecDeque;
use strum::IntoEnumIterator;

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

async fn blocking_movement<G: Gamepad>(
    gamepad: &mut G,
    packet: &MovementPacket,
) -> anyhow::Result<()> {
    let MovementPacket {
        movements,
        duration,
        stagger,
        ..
    } = packet;

    for movement in movements.iter() {
        gamepad.press(*movement)?;

        if *stagger != 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(*stagger)).await;
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(*duration)).await;

    for movement in movements.iter().rev() {
        gamepad.release(*movement)?;

        if *stagger != 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(*stagger)).await;
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    Ok(())
}

struct RunnerState<'a, G: Gamepad> {
    update_interval_ms: u64,
    gamepad: &'a mut G,
    movement_time_remaining: Box<[u64]>,
    packet_queue: VecDeque<MovementPacket>,
    interval: tokio::time::Interval,
    draining: bool,
}

impl<'a, G: Gamepad> RunnerState<'a, G> {
    fn time_remaining_empty(&self) -> bool {
        self.movement_time_remaining
            .iter()
            .all(|remaining| *remaining == 0)
    }

    fn cancel_if_active(&mut self, movement: Movement) -> anyhow::Result<()> {
        if self.movement_time_remaining[movement as usize] > 0 {
            self.movement_time_remaining[movement as usize] = 0;
            self.gamepad.release(movement)?;
        }

        Ok(())
    }

    fn cancel_directional(&mut self) -> anyhow::Result<()> {
        self.cancel_if_active(Movement::Up)?;
        self.cancel_if_active(Movement::Down)?;
        self.cancel_if_active(Movement::Left)?;
        self.cancel_if_active(Movement::Right)?;
        Ok(())
    }

    fn packet_can_run(&self, packet: &MovementPacket) -> bool {
        packet
            .movements
            .iter()
            .all(|movement| self.movement_time_remaining[*movement as usize] == 0)
    }

    async fn process_packet(
        &mut self,
        packet: &MovementPacket,
        ticking: bool,
    ) -> anyhow::Result<bool> {
        if packet.blocking {
            if self.time_remaining_empty() {
                blocking_movement(self.gamepad, packet).await?;
                return Ok(true);
            } else {
                return Ok(false);
            }
        }

        if !ticking && !self.packet_queue.is_empty() {
            return Ok(false);
        }

        // If a packet contains a direction, give it priority
        if packet.contains_direction() {
            println!("contained direction");
            self.cancel_directional()?;

            for movement in packet.movements.iter() {
                self.cancel_if_active(*movement)?;
                // FIXME: need to wait after release
                self.gamepad.press(*movement)?;
                self.movement_time_remaining[*movement as usize] = packet.duration;
            }

            return Ok(true);
        }

        if self.packet_can_run(packet) {
            for movement in packet.movements.iter() {
                self.gamepad.press(*movement)?;
                self.movement_time_remaining[*movement as usize] = packet.duration;
            }

            return Ok(true);
        }

        Ok(false)
    }

    async fn process_message(&mut self, msg: Option<MovementPacket>) -> anyhow::Result<()> {
        let packet: MovementPacket = match msg {
            Some(packet) => packet,
            None => {
                self.draining = true;
                return Ok(());
            }
        };

        println!("received packet {:?}", packet);
        let processed = self.process_packet(&packet, false).await?;
        if !processed {
            println!("pushing packet {:?}", packet);
            self.packet_queue.push_back(packet);
        }

        Ok(())
    }

    async fn process_tick(&mut self) -> anyhow::Result<bool> {
        let mut all_zero = true;
        println!("before tick: {:?}", self.movement_time_remaining);
        for movement in Movement::iter() {
            let time_remaining = &mut self.movement_time_remaining[movement as usize];
            if *time_remaining == 0 {
                continue;
            }

            all_zero = false;
            *time_remaining = time_remaining.saturating_sub(self.update_interval_ms);

            if *time_remaining == 0 {
                self.gamepad.release(movement)?;
            }
        }

        println!("after tick: {:?}", self.movement_time_remaining);

        if all_zero {
            while let Some(packet) = self.packet_queue.pop_front() {
                println!("popped {:?}", packet);
                if !self.process_packet(&packet, true).await? {
                    println!("didnt process it, pushing {:?}", packet);
                    self.packet_queue.push_front(packet);
                    break;
                }
            }
        }

        // all_zero is no longer valid here, we may have mutated the remaining time
        if self.draining && self.time_remaining_empty() && self.packet_queue.is_empty() {
            println!("drained");
            return Ok(true);
        }

        Ok(false)
    }
}

pub async fn gamepad_runner<G: Gamepad>(
    gamepad: &mut G,
    mut rx: Receiver<MovementPacket>,
) -> anyhow::Result<()> {
    let update_interval_ms = 100;
    let mut runner_state = RunnerState {
        update_interval_ms,
        gamepad,
        movement_time_remaining: vec![0; Movement::iter().count()].into_boxed_slice(),
        packet_queue: VecDeque::new(),
        interval: tokio::time::interval(tokio::time::Duration::from_millis(update_interval_ms)),
        draining: false,
    };

    loop {
        select! {
            msg = rx.recv() => {
                runner_state.process_message(msg).await?;
            },
            _ = runner_state.interval.tick() => {
                if runner_state.process_tick().await? {
                    break Ok(());
                }
            }
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
