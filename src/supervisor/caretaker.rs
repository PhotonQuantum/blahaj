use std::collections::VecDeque;
use std::future::Future;
use std::process::Stdio;
use std::time::Duration;

use actix::dev::{MessageResponse, OneshotSender};
use actix::fut::{ready, wrap_future};
use actix::{
    Actor, ActorFutureExt, Addr, AsyncContext, Context, Handler, Message, ResponseActFuture,
    ResponseFuture, WrapFuture,
};
use actix_signal::AddrSignalExt;
use awc::http::uri::Scheme;
use awc::http::Uri;
use awc::Client;
use futures::future::Either;
use futures::{FutureExt, Stream, StreamExt};
use log::Level;
use tokio::io::AsyncRead;
use tokio::process::{Child, Command};
use tokio::sync::oneshot::Receiver;
use tokio::sync::{broadcast, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::config::Program;
use crate::error::SupervisorError;
use crate::logger::{ChildOutput, Custom, IOType, LoggerActor, RegisterStdio};
use crate::supervisor::futs::{RepeatedActFut, WaitAbortFut, WaitAbortResult};
use crate::supervisor::health::{HealthActor, HealthConfig};
use crate::supervisor::retry::RetryGuard;
use crate::supervisor::terminate_ext::TerminateExt;

/// Wraps a stdio pipe into an event stream.
fn wrap_stream(
    stdio: impl AsyncRead,
    name: String,
    io_type: IOType,
) -> impl Stream<Item = ChildOutput> {
    FramedRead::new(stdio, LinesCodec::new_with_max_length(1024)).filter_map(move |item| {
        let name = name.clone();
        async move {
            item.ok().map(move |line| ChildOutput {
                name: name.clone(),
                ty: io_type,
                data: line,
            })
        }
    })
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HealthState {
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Lifecycle {
    /// Process is starting.
    Starting,
    /// Process is running.
    Running(HealthState),
    /// Process is being terminated.
    Terminating,
    /// Process is terminated.
    Terminated,
    /// Process has died and no further retry is scheduled.
    Failed,
}

impl<A, M> MessageResponse<A, M> for Lifecycle
where
    A: Actor,
    M: Message<Result = Self>,
{
    fn handle(self, _: &mut A::Context, tx: Option<OneshotSender<M::Result>>) {
        if let Some(tx) = tx {
            let _ = tx.send(self);
        }
    }
}

/// Handles the lifecycle of a child, and book-keeping its status.
pub struct CaretakerActor {
    name: String,
    program: Program,
    status: Lifecycle,
    client: Client,
    stop_tx: Option<oneshot::Sender<()>>,
    stopped_tx: Option<broadcast::Sender<()>>,
    logger: Addr<LoggerActor>,
}

impl CaretakerActor {
    pub const fn new(
        name: String,
        program: Program,
        client: Client,
        logger: Addr<LoggerActor>,
    ) -> Self {
        Self {
            name,
            program,
            status: Lifecycle::Starting,
            client,
            stop_tx: None,
            stopped_tx: None,
            logger,
        }
    }
    pub fn broadcast_stopped(&mut self) {
        self.stop_tx = None;
        self.stopped_tx
            .take()
            .unwrap()
            .send(())
            .expect("send stopped_tx signal");
    }
}

fn build_child(program: &Program) -> Result<Child, SupervisorError> {
    let cmd = &program.command;
    let mut command = Command::new(&cmd.cmd);
    command
        .args(&cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (var, value) in &program.env {
        match &value.0 {
            None => command.env_remove(var),
            Some(s) => command.env(var, s),
        };
    }
    command.spawn().map_err(SupervisorError::Spawn)
}

/// Run this program.
/// # Returns
/// `Ok(Some(stop_rx))` if the program ends itself.
/// `Ok(None)` if the program is ended by terminate signal.
/// `Err((stop_rx, e))` if child can't be spawned, exited too quickly, or returned a non-zero exit status.
#[derive(Message)]
#[rtype("RunResult")]
pub struct Run(oneshot::Receiver<()>);

pub enum RunResult {
    NormalExit(oneshot::Receiver<()>),
    Terminated,
    Unhealthy(oneshot::Receiver<()>),
    Error(ErrType),
}

type ErrType = (oneshot::Receiver<()>, SupervisorError);
type FutOutputType = <WaitAbortFut as Future>::Output;

enum SpawnRunOutput {
    Success(FutOutputType),
    Failure(ErrType),
}

async fn ensure_stop(child: &mut Child, grace: Duration) {
    if tokio::time::timeout(grace, child.terminate())
        .await
        .is_err()
    {
        // Child failed to exit in time. Kill it.
        drop(child.kill().await);
    }
}

fn uri_for_check(https: bool, port: u16, path: &str) -> Uri {
    Uri::builder()
        .scheme(if https { Scheme::HTTPS } else { Scheme::HTTP })
        .authority(format!("127.0.0.1:{}", port))
        .path_and_query(path)
        .build()
        .expect("must build")
}

impl Handler<Run> for CaretakerActor {
    type Result = ResponseActFuture<Self, RunResult>;

    fn handle(&mut self, msg: Run, ctx: &mut Self::Context) -> Self::Result {
        let stop_rx = msg.0;
        let program = self.program.clone();

        let logger = self.logger.clone();
        let name = self.name.clone();

        let grace = self.program.grace;

        // Spawn the child.
        self.status = Lifecycle::Starting;
        match build_child(&program) {
            Ok(mut child) => {
                // Child spawn successful, register logger and set status.
                let stdout = wrap_stream(
                    child.stdout.take().expect("child stdout"),
                    name.clone(),
                    IOType::Stdout,
                );
                let stderr = wrap_stream(
                    child.stderr.take().expect("child stderr"),
                    name,
                    IOType::Stderr,
                );
                logger.do_send(RegisterStdio(stdout));
                logger.do_send(RegisterStdio(stderr));

                // Setup health actor.
                let (unhealthy_tx, unhealthy_rx) = oneshot::channel();
                let config = self
                    .program
                    .http
                    .as_ref()
                    .and_then(|http| http.health_check.as_ref().map(|check| (http, check)))
                    .map(|(http, check)| {
                        self.status = Lifecycle::Running(HealthState::Unknown);
                        let uri = uri_for_check(http.https, http.port, check.path.as_str());
                        HealthConfig {
                            uri,
                            interval: check.interval,
                            grace_period: check.grace,
                        }
                    });
                let health_actor = HealthActor::new(
                    self.name.clone(),
                    self.client.clone(),
                    config,
                    unhealthy_tx,
                    ctx.address(),
                    self.logger.clone(),
                );
                let health_addr = health_actor.start();

                // Handle the child and stop handler to WaitAbortFut.
                Either::Left(
                    WaitAbortFut::new(child, stop_rx, unhealthy_rx)
                        .map(SpawnRunOutput::Success)
                        .map(move |res| (res, Some(health_addr))),
                )
            }
            // Failed to spawn child, return error.
            Err(e) => Either::Right(ready((SpawnRunOutput::Failure((stop_rx, e)), None))),
        }
        .into_actor(self)
        .map(|(res, health_addr), act, _| {
            // WaitAbortFut resolves (or child spawn error).
            if let Some(health_addr) = health_addr {
                health_addr.stop(); // Stop health actor.
            }

            if matches!(
                res,
                SpawnRunOutput::Success(
                    WaitAbortResult::StopSignalReceived(_)
                        | WaitAbortResult::UnhealthySignalReceived(_, _)
                )
            ) {
                // Fut resolves because it received a stop signal.
                act.status = Lifecycle::Terminating;
            }
            res
        })
        .then(move |res, act, _| {
            async move {
                match res {
                    SpawnRunOutput::Success(WaitAbortResult::StopSignalReceived(mut child)) => {
                        // Fut resolves because it received a stop signal. Stop the child.
                        ensure_stop(&mut child, grace).await;
                        RunResult::Terminated
                    }
                    SpawnRunOutput::Success(WaitAbortResult::UnhealthySignalReceived(
                        stop_rx,
                        mut child,
                    )) => {
                        // Fut resolves because it received a unhealthy signal. Stop the child.
                        ensure_stop(&mut child, grace).await;
                        RunResult::Unhealthy(stop_rx)
                    }
                    // Fut resolves because the process exits without error. Retrieve the stop handler.
                    SpawnRunOutput::Success(WaitAbortResult::Exit(stop_rx)) => {
                        RunResult::NormalExit(stop_rx)
                    }
                    // Fut resolves because the process exits with an error. Retrieve the stop handler and error msg.
                    SpawnRunOutput::Success(WaitAbortResult::ExitWithError(e))
                    | SpawnRunOutput::Failure(e) => RunResult::Error(e),
                }
            }
            .into_actor(act)
        })
        .map(|res, act, _| {
            // Process now died.
            act.status = Lifecycle::Terminated;
            res
        })
        .boxed_local()
    }
}

/// Wait for the child to exit.
#[derive(Message)]
#[rtype("()")]
pub struct Wait;

impl Handler<Wait> for CaretakerActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _: Wait, _: &mut Self::Context) -> Self::Result {
        self.stopped_tx.as_ref().map_or_else(
            || Box::pin(ready(())) as Self::Result,
            |stopped_tx| {
                // The child is terminating, wait for stopped_rx.
                let mut stopped_rx = stopped_tx.subscribe();
                Box::pin(async move {
                    drop(stopped_rx.recv().await);
                })
            },
        )
    }
}

/// Send terminate signal to child, and wait until it exits.
#[derive(Message)]
#[rtype("()")]
pub struct Terminate;

impl Handler<Terminate> for CaretakerActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _: Terminate, ctx: &mut Self::Context) -> Self::Result {
        if let Some(stop_tx) = self.stop_tx.take() {
            // The child is still running, wait for stopped_rx.
            stop_tx.send(()).expect("send stop signal");
        }
        ctx.address().send(Wait).map(|_| ()).boxed_local()
    }
}

/// Keep the program running until retry limits reached or terminate signal received.
#[derive(Message)]
#[rtype("()")]
pub struct Daemon(oneshot::Receiver<()>);

impl Handler<Daemon> for CaretakerActor {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Daemon, ctx: &mut Self::Context) -> Self::Result {
        let stop_rx = msg.0;
        let addr = ctx.address();
        let one_round_fut = move |stop_rx: Receiver<()>| {
            wrap_future::<_, Self>(addr.send(Run(stop_rx))).map(|res, act, _| {
                let res = res.expect("self is alive");
                match res {
                    RunResult::NormalExit(stop_rx) => {
                        act.logger
                            .do_send(Custom::new(act.name.clone(), "Exits without error."));
                        Some(stop_rx)
                    }
                    RunResult::Unhealthy(stop_rx) => {
                        act.logger.do_send(Custom::new(
                            act.name.clone(),
                            String::from("Terminated by caretaker."),
                        ));
                        Some(stop_rx)
                    }
                    RunResult::Terminated => {
                        act.logger.do_send(Custom::new(
                            act.name.clone(),
                            String::from("Terminated by caretaker."),
                        ));
                        None
                    }
                    RunResult::Error((stop_rx, e)) => {
                        act.logger.do_send(Custom::new(
                            act.name.clone(),
                            format!("Exits with error: {}.", e),
                        ));
                        Some(stop_rx)
                    }
                }
            })
        };

        let retry_guard = RetryGuard::new(
            VecDeque::new(),
            self.program.retry.window,
            self.program.retry.count,
        );
        RepeatedActFut::new(one_round_fut, stop_rx, retry_guard)
            .map(|res, act, _| {
                if !res {
                    act.status = Lifecycle::Failed;
                    act.logger.do_send(Custom::new_with_level(
                        Level::Error,
                        act.name.clone(),
                        String::from("Retry limits exceeded, giving up"),
                    ));
                }
                act.broadcast_stopped();
            })
            .boxed_local()
    }
}

/// Notify the caretaker to update running status.
#[derive(Debug, Message)]
#[rtype("()")]
pub struct SetHealthState(pub HealthState);

impl Handler<SetHealthState> for CaretakerActor {
    type Result = ();

    fn handle(&mut self, msg: SetHealthState, _ctx: &mut Self::Context) -> Self::Result {
        if matches!(self.status, Lifecycle::Running(_) | Lifecycle::Starting) {
            self.status = Lifecycle::Running(msg.0);
        }
    }
}

/// Notify the caretaker to update running status.
#[derive(Debug, Message)]
#[rtype("Lifecycle")]
pub struct GetStatus;

impl Handler<GetStatus> for CaretakerActor {
    type Result = Lifecycle;

    fn handle(&mut self, _: GetStatus, _: &mut Self::Context) -> Self::Result {
        self.status
    }
}

impl Actor for CaretakerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (stopped_tx, _) = broadcast::channel(10);
        self.stopped_tx = Some(stopped_tx);
        self.stop_tx = Some(stop_tx);
        ctx.notify(Daemon(stop_rx));
    }
}
