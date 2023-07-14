use crate::config::GameCommand;
use nix::sys::signal::{kill, Signal};
use tokio::process::Child;
use tracing::info;

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

async fn stop_child(child: &mut Option<Child>) -> anyhow::Result<()> {
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

        info!("Child should be gone now");
    }

    Ok(())
}

async fn game_runner_loop(mut rx: tokio::sync::mpsc::Receiver<GameRunner>) -> anyhow::Result<()> {
    let mut current_process: Option<tokio::process::Child> = None;

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(GameRunner::Stop) => stop_child(&mut current_process).await?,
                    Some(GameRunner::SwitchTo(gc)) => {
                        stop_child(&mut current_process).await?;
                        current_process = Some(tokio::process::Command::new(gc.command)
                            .args(gc.args)
                            .spawn()?)
                    }
                    _ => {
                        tracing::info!("Game runner done");
                        stop_child(&mut current_process).await?;
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

    let handle = tokio::task::spawn(async move {
        game_runner_loop(rx).await?;
        Ok(())
    });

    (handle, tx)
}
