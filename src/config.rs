use anyhow::anyhow;
use serde::{de::Error, Deserialize, Deserializer};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use crate::command::{parse_movement_token, Movement, MovementPacket};

fn deserialize_u64_map<'d, D, T>(deserializer: D) -> Result<BTreeMap<u64, T>, D::Error>
where
    D: Deserializer<'d>,
    T: Deserialize<'d>,
{
    let orig_map: BTreeMap<String, T> = BTreeMap::deserialize(deserializer)?;
    let u64_key_map: Result<BTreeMap<u64, T>, D::Error> = orig_map
        .into_iter()
        .map(|(k, v)| {
            k.parse::<u64>()
                .map(|k| (k, v))
                .map_err(|e| D::Error::custom(format!("Failed to parse u64 key: {:?}", e)))
        })
        .collect();
    u64_key_map
}

pub type GameName = String;

#[derive(Clone, Deserialize)]
pub struct GameCommandString(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameCommand {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone)]
pub struct ConstructedGameInfo {
    pub name: String,
    pub command: GameCommand,
    pub restricted_inputs: HashSet<Movement>,
    pub controls_msg: Option<String>,
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
pub struct GameInfo {
    pub command: GameCommandString,
    pub restricted_inputs: Option<Vec<String>>,
    pub controls: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct SoundEffectConfig {
    pub command: String,
    pub sounds: BTreeMap<String, String>,

    #[serde(deserialize_with = "deserialize_u64_map")]
    pub sub_events: BTreeMap<u64, String>,
}

#[derive(Clone, Deserialize)]
pub struct Config {
    pub twitch: TwitchConfig,
    pub sound_effects: Option<SoundEffectConfig>,
    pub games: Option<BTreeMap<GameName, GameInfo>>,
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

impl ConstructedGameInfo {
    pub fn is_movement_restricted(&self, packet: &MovementPacket) -> bool {
        for movement in packet.movements.iter() {
            if self.restricted_inputs.contains(movement) {
                return true;
            }
        }

        false
    }
}

impl Config {
    pub fn game_command_list(&self) -> BTreeMap<GameName, ConstructedGameInfo> {
        self.games
            .as_ref()
            .map(|games| {
                games
                    .iter()
                    .map(|(name, gi)| {
                        let mut ri = HashSet::new();
                        if let Some(ref restricted_inputs) = gi.restricted_inputs {
                            for m in restricted_inputs.iter() {
                                let m = m.to_lowercase();
                                let m =
                                    parse_movement_token(&m).expect("invalid restricted movement");
                                ri.insert(m);
                            }
                        }

                        (
                            name.to_owned(),
                            ConstructedGameInfo {
                                name: name.to_owned(),
                                command: gi.command.to_command(),
                                restricted_inputs: ri,
                                controls_msg: gi.controls.clone(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
