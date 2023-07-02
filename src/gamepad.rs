use crate::command::Movement;
use uinput::event::{absolute, controller};

pub trait Gamepad {
    fn press(&mut self, movement: &Movement) -> anyhow::Result<()>;
    fn release(&mut self, movement: &Movement) -> anyhow::Result<()>;
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
        }
    }
}

impl Gamepad for UinputGamepad {
    fn press(&mut self, movement: &Movement) -> anyhow::Result<()> {
        let cmd = Self::map_movement(movement);

        self.gamepad.press(&cmd).unwrap();
        self.gamepad.synchronize().unwrap();
        Ok(())
    }

    fn release(&mut self, movement: &Movement) -> anyhow::Result<()> {
        let cmd = Self::map_movement(movement);

        self.gamepad.release(&cmd).unwrap();
        self.gamepad.synchronize().unwrap();
        Ok(())
    }
}
