use anyhow::anyhow;
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

pub type GameName = String;

#[derive(Clone, Deserialize)]
pub struct GameCommandString(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameCommand {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", content = "credentials")]
pub enum TwitchAuth {
    Anonymous,
    Login {
        client: String,
        secret: String,
        access: Option<String>,
    },
}

#[derive(Clone, Deserialize)]
pub struct TwitchConfig {
    pub channel_name: String,
    pub auth: TwitchAuth,
}

#[derive(Clone, Deserialize)]
pub struct Config {
    pub twitch: TwitchConfig,
    pub games: Option<BTreeMap<GameName, GameCommandString>>,
}

fn cfg_path() -> anyhow::Result<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(path) = args.get(1) {
        return Ok(PathBuf::from(path));
    }

    let mut curdir = std::env::current_dir()?;
    loop {
        let cfg_path = curdir.join("twitch_gamepad.toml");
        if cfg_path.is_file() {
            tracing::info!("Found config file: {:?}", cfg_path);
            break Ok(cfg_path);
        }

        match curdir.parent() {
            Some(parent) => curdir = parent.to_owned(),
            None => break Err(anyhow!("No configuration file found")),
        }
    }
}

pub async fn read_config() -> anyhow::Result<(Config, PathBuf)> {
    let cfg_path = cfg_path()?;
    let cfg = tokio::fs::read_to_string(&cfg_path).await?;
    let cfg: Config = toml::from_str(&cfg)?;
    Ok((cfg, cfg_path))
}

impl GameCommandString {
    pub fn to_command(&self) -> GameCommand {
        let mut args = self.0.split(' ').map(|s| s.to_owned());
        let command = args.next().expect("game command should include a command");
        let args: Vec<String> = args.collect();

        GameCommand { command, args }
    }
}

impl Config {
    pub fn game_command_list(&self) -> BTreeMap<GameName, GameCommand> {
        self.games
            .as_ref()
            .map(|games| {
                games
                    .iter()
                    .map(|(name, cmd)| (name.to_owned(), cmd.to_command()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
