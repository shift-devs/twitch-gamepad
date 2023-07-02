use anyhow::anyhow;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Deserialize)]
pub struct TwitchConfig {
    pub channel_name: String,
}

#[derive(Clone, Deserialize)]
pub struct Config {
    pub twitch: TwitchConfig,
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
