use crate::config::{GameCommand, SoundEffectConfig};
use nix::sys::signal::{kill, Signal};
use tokio::process::{Child, Command};
use tracing::{info, warn};

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameRunner {
    SwitchTo(GameCommand),
    Stop,
}

async fn wait_on_child(child: &mut Option<Child>) -> anyhow::Result<()> {
    if let Some(child) = child {
        child.wait().await?;
        Ok(())
    } else {
        // Never resolve so that we don't spin excessively
        std::future::pending::<()>().await;
        unreachable!("Pending resolved")
    }
}

async fn stop_child(
    child: &mut Option<Child>,
    child_pid_atomic: &Arc<AtomicI32>,
) -> anyhow::Result<()> {
    if let Some(child) = child {
        info!("Exiting current child");
        if let Some(pid) = child.id() {
            let pid = nix::unistd::Pid::from_raw(pid as i32);
            info!("Sending sigterm");
            kill(pid, Signal::SIGTERM)?;
            child.wait().await?;
        } else {
            info!("Killing process");
            child.kill().await?;
        }

        child_pid_atomic.store(0, Ordering::Relaxed);
        info!("Child should be gone now");
    }

    Ok(())
}

async fn game_runner_loop(
    mut rx: tokio::sync::mpsc::Receiver<GameRunner>,
    child_pid_atomic: Arc<AtomicI32>,
) -> anyhow::Result<()> {
    let mut current_process: Option<tokio::process::Child> = None;

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(GameRunner::Stop) => stop_child(&mut current_process, &child_pid_atomic).await?,
                    Some(GameRunner::SwitchTo(gc)) => {
                        stop_child(&mut current_process, &child_pid_atomic).await?;
                        let new_process = tokio::process::Command::new(gc.command)
                            .args(gc.args)
                            .spawn()?;
                        child_pid_atomic.store(match new_process.id() {
                            Some(pid) => pid as i32,
                            None => 0,
                        }, Ordering::Relaxed);
                        current_process = Some(new_process);
                    }
                    _ => {
                        tracing::info!("Game runner done");
                        stop_child(&mut current_process, &child_pid_atomic).await?;
                        break Ok(());
                    },
                }
            },
            _ = wait_on_child(&mut current_process) => {
                info!("Child exited");
                current_process = None;
            },
        }
    }
}

pub fn run_game_runner() -> (
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tokio::sync::mpsc::Sender<GameRunner>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(20);

    let child_pid_atomic = Arc::new(AtomicI32::new(0));

    {
        let child_pid_atomic = child_pid_atomic.clone();
        let panic_handler = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            eprintln!("Panic handler invoked, killing child process");
            let pid = child_pid_atomic.load(Ordering::Relaxed);
            if pid != 0 {
                eprintln!("Sending kill signal to {}", pid);
                let pid = nix::unistd::Pid::from_raw(pid);
                let _ignored = kill(pid, Signal::SIGTERM);
            }

            panic_handler(panic_info);
        }));
    }

    let handle = tokio::task::spawn(async move {
        game_runner_loop(rx, child_pid_atomic).await?;
        Ok(())
    });

    (handle, tx)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SfxRequest {
    SubEvent(u64),
    Named(String),
    Enable(bool),
}

impl SfxRequest {
    fn to_file<'a>(&self, cfg: &'a SoundEffectConfig) -> Option<&'a String> {
        match self {
            Self::SubEvent(count) => cfg
                .sub_events
                .range(..=count)
                .next_back()
                .and_then(|(_, sfx_name)| cfg.sounds.get(sfx_name)),
            Self::Named(sfx) => cfg.sounds.get(sfx),
            _ => None,
        }
    }
}

async fn sound_effect_runner(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<SfxRequest>,
    cfg: &SoundEffectConfig,
) -> anyhow::Result<()> {
    let mut is_enabled = true;
    info!("Started SFX runner");

    for (event, sfx) in cfg.sub_events.iter() {
        let sfx = cfg.sounds.get(sfx);
        info!("at least {} subs will play {:?}", event, sfx);

        if let Some(file) = sfx {
            let exists = std::path::PathBuf::from(file).exists();
            info!(
                "File {} {}",
                file,
                match exists {
                    true => "exists",
                    false => "does not exist",
                }
            );
        }
    }

    while let Some(effect) = rx.recv().await {
        if let SfxRequest::Enable(en) = effect {
            info!("Setting SFX to {:?}", effect);
            is_enabled = en;
            continue;
        }

        if let Some(sfx_file) = effect.to_file(cfg) {
            if !is_enabled {
                info!("SFX disabled, skipping");
                continue;
            }

            info!("Playing sound effect for {:?}", effect);
            Command::new(cfg.command.clone())
                .args(vec![sfx_file, "--fullscreen"])
                .spawn()?;
        } else {
            warn!("No sound effect file supplied for effect {:?}", effect);
        }
    }

    Ok(())
}

pub fn run_sfx_runner(
    cfg: SoundEffectConfig,
) -> (
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tokio::sync::mpsc::UnboundedSender<SfxRequest>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::task::spawn(async move { sound_effect_runner(rx, &cfg).await });

    (handle, tx)
}

#[cfg(test)]
mod sfx_player {
    use std::collections::BTreeMap;

    use crate::config::SoundEffectConfig;

    #[test]
    fn test_sub_events() {
        let mut sub_events = BTreeMap::new();
        sub_events.insert(20, "20".to_owned());
        sub_events.insert(60, "60".to_owned());
        sub_events.insert(80, "80".to_owned());
        sub_events.insert(100, "100".to_owned());

        let mut sounds = BTreeMap::new();
        sounds.insert("20".to_owned(), "20".to_owned());
        sounds.insert("60".to_owned(), "60".to_owned());
        sounds.insert("80".to_owned(), "80".to_owned());
        sounds.insert("100".to_owned(), "100".to_owned());

        let cfg = SoundEffectConfig {
            command: "cmd".to_owned(),
            sounds,
            sub_events,
        };

        use super::SfxRequest::SubEvent;
        assert_eq!(SubEvent(10).to_file(&cfg), None);
        assert_eq!(SubEvent(20).to_file(&cfg), Some(&"20".to_owned()));
        assert_eq!(SubEvent(30).to_file(&cfg), Some(&"20".to_owned()));
        assert_eq!(SubEvent(60).to_file(&cfg), Some(&"60".to_owned()));
        assert_eq!(SubEvent(70).to_file(&cfg), Some(&"60".to_owned()));
        assert_eq!(SubEvent(80).to_file(&cfg), Some(&"80".to_owned()));
        assert_eq!(SubEvent(99).to_file(&cfg), Some(&"80".to_owned()));
        assert_eq!(SubEvent(100).to_file(&cfg), Some(&"100".to_owned()));
        assert_eq!(SubEvent(2147483647).to_file(&cfg), Some(&"100".to_owned()));
    }
}
