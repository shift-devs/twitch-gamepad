use crate::config::{GameCommand, SoundEffect, SoundEffectConfig};
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
    Event(SoundEffect),
    Named(String),
    Enable(bool),
}

impl SfxRequest {
    fn to_file<'a>(&self, cfg: &'a SoundEffectConfig) -> Option<&'a String> {
        match self {
            Self::Event(effect) => cfg
                .event_map
                .get(effect)
                .and_then(|sfx_name| cfg.sounds.get(sfx_name)),
            Self::Named(sfx) => cfg.sounds.get(sfx),
            _ => None,
        }
    }
}

async fn sound_effect_runner(
    mut rx: tokio::sync::mpsc::Receiver<SfxRequest>,
    cfg: &SoundEffectConfig,
) -> anyhow::Result<()> {
    let mut is_enabled = true;
    info!("Started SFX runner");

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
                .spawn()?
                .wait()
                .await?;
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
    tokio::sync::mpsc::Sender<SfxRequest>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(50);
    let handle = tokio::task::spawn(async move { sound_effect_runner(rx, &cfg).await });

    (handle, tx)
}
