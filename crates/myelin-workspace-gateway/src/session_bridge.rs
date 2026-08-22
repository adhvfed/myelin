use std::fs::File;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::time::Duration;

use myelin_ci_sandbox::gvisor::{ConfinedWorkspaceSession, ConfinedWorkspaceSessionIo};
use russh::server::Msg;
use russh::Channel;
use tokio::io::AsyncWriteExt as _;

use crate::{AuthenticatedWorkspace, WorkspaceSshAdmissionStore, WorkspaceSshAuthenticator};

pub(crate) fn spawn_session_bridge<A>(
    mut confined: ConfinedWorkspaceSession,
    channel: Channel<Msg>,
    authenticator: WorkspaceSshAuthenticator<A>,
    authenticated: AuthenticatedWorkspace,
) where
    A: WorkspaceSshAdmissionStore,
{
    tokio::spawn(async move {
        let Ok(io) = confined.take_io() else {
            return;
        };
        let session_handle = confined.handle();
        let mut wait = tokio::task::spawn_blocking(move || confined.wait());
        let (mut channel_input, channel_output) = channel.split();
        let ConfinedWorkspaceSessionIo {
            stdin,
            stdout,
            stderr,
        } = io;
        let mut child_input = async_pipe(stdin);
        let mut child_output = async_pipe(stdout);
        let mut child_error = async_pipe(stderr);
        let mut output_destination = channel_output.make_writer();
        let mut error_destination = channel_output.make_writer_ext(Some(1));

        let input_handle = session_handle.clone();
        let input = tokio::spawn(async move {
            let result = tokio::io::copy(&mut channel_input.make_reader(), &mut child_input).await;
            let _ = child_input.shutdown().await;
            if result.is_err() {
                input_handle.terminate();
            }
        });
        let output_handle = session_handle.clone();
        let mut output = tokio::spawn(async move {
            let result = tokio::io::copy(&mut child_output, &mut output_destination).await;
            if result.is_err() {
                output_handle.terminate();
            }
        });
        let error_handle = session_handle.clone();
        let mut error = tokio::spawn(async move {
            let result = tokio::io::copy(&mut child_error, &mut error_destination).await;
            if result.is_err() {
                error_handle.terminate();
            }
        });

        let mut recheck = tokio::time::interval(Duration::from_secs(15));
        recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        recheck.tick().await;
        let exit = loop {
            tokio::select! {
                result = &mut wait => break result,
                _ = recheck.tick() => {
                    match authenticator.revalidate(&authenticated).await {
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => session_handle.terminate(),
                    }
                }
            }
        };

        input.abort();
        let _ = input.await;
        drain_or_abort(&mut output).await;
        drain_or_abort(&mut error).await;
        let code = exit
            .ok()
            .and_then(Result::ok)
            .and_then(|exit| exit.code)
            .and_then(|code| u32::try_from(code).ok())
            .unwrap_or(255);
        let _ = channel_output.exit_status(code).await;
        let _ = channel_output.eof().await;
        let _ = channel_output.close().await;
    });
}

async fn drain_or_abort(task: &mut tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(Duration::from_secs(1), &mut *task)
        .await
        .is_err()
    {
        task.abort();
    }
}

fn async_pipe(pipe: impl IntoRawFd) -> tokio::fs::File {
    let file = unsafe { File::from_raw_fd(pipe.into_raw_fd()) };
    tokio::fs::File::from_std(file)
}
