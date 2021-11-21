use std::collections::VecDeque;
use std::env;
use std::future::Future;
use std::process::Stdio;
use std::time::Duration;

use actix::fut::{ready, wrap_future};
use actix::{
    Actor, ActorFutureExt, Addr, AsyncContext, Context, Handler, Message, ResponseActFuture,
    ResponseFuture, WrapFuture,
};
use futures::future::Either;
use futures::{FutureExt, Stream, StreamExt};
use log::Level;
use tokio::io::AsyncRead;
use tokio::process::{Child, Command};
use tokio::sync::oneshot::Receiver;
use tokio::sync::{broadcast, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::config::{Env, Program};
use crate::error::Error;
use crate::logger::{ChildOutput, Custom, IOType, LoggerActor, RegisterStdio};
use crate::supervisor::futs::{RepeatedActFut, WaitAbortFut};
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
    Starting,
    Running(HealthState),
    Terminating,
    Terminated,
}

/// Handles the lifecycle of a child, and book-keeping its status.
#[derive(Debug)]
pub struct ProgramActor {
    name: String,
    program: Program,
    status: Lifecycle,
    stop_tx: Option<oneshot::Sender<()>>,
    stopped_tx: Option<broadcast::Sender<()>>,
    logger: Addr<LoggerActor>,
}

impl ProgramActor {
    pub const fn new(name: String, program: Program, logger: Addr<LoggerActor>) -> Self {
        Self {
            name,
            program,
            status: Lifecycle::Terminated,
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

fn build_child(program: &Program) -> Result<Child, Error> {
    let cmd = &program.command;
    let mut command = Command::new(&cmd.cmd);
    command
        .args(&cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (var, value) in &program.env {
        match value {
            None => command.env_remove(var),
            Some(Env::Literal(s)) => command.env(var, s),
            Some(Env::Inherit(inherited)) => command.env(var, env::var(inherited)?),
        };
    }
    command.spawn().map_err(Error::SpawnError)
}

/// Run this program.
/// # Returns
/// `Ok(Some(stop_rx))` if the program ends itself.
/// `Ok(None)` if the program is ended by terminate signal.
/// `Err((stop_rx, e))` if child can't be spawned, exited too quickly, or returned a non-zero exit status.
#[derive(Message)]
#[rtype("Result<Option<oneshot::Receiver<()>>, (oneshot::Receiver<()>, Error)>")]
pub struct Run(oneshot::Receiver<()>);

type OKType = Option<oneshot::Receiver<()>>;
type ErrType = (oneshot::Receiver<()>, Error);
type FutOutputType = <WaitAbortFut as Future>::Output;

enum SpawnRunOutput {
    Success(FutOutputType),
    Failure(ErrType),
}

impl Handler<Run> for ProgramActor {
    type Result = ResponseActFuture<Self, Result<OKType, ErrType>>;

    fn handle(&mut self, msg: Run, _: &mut Self::Context) -> Self::Result {
        let stop_rx = msg.0;
        let program = self.program.clone();

        let logger = self.logger.clone();
        let name = self.name.clone();

        self.status = Lifecycle::Starting;
        match build_child(&program) {
            Ok(mut child) => {
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
                self.status = Lifecycle::Running(HealthState::Unknown);
                Either::Left(WaitAbortFut::new(child, stop_rx).map(SpawnRunOutput::Success))
            }
            Err(e) => Either::Right(ready(SpawnRunOutput::Failure((stop_rx, e)))),
        }
        .into_actor(self)
        .map(|res: SpawnRunOutput, act, _| {
            if matches!(res, SpawnRunOutput::Success(Ok(Err(_)))) {
                act.status = Lifecycle::Terminating;
            }
            res
        })
        .then(|res, act, _| {
            async move {
                match res {
                    SpawnRunOutput::Success(Ok(Err(mut child))) => {
                        if tokio::time::timeout(Duration::from_secs(5), child.terminate())
                            .await
                            .is_err()
                        {
                            drop(child.kill().await);
                        }
                        Ok(None)
                    }
                    SpawnRunOutput::Success(Ok(Ok(stop_rx))) => Ok(Some(stop_rx)),
                    SpawnRunOutput::Success(Err(e)) | SpawnRunOutput::Failure(e) => Err(e),
                }
            }
            .into_actor(act)
        })
        .map(|res, act, _| {
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

impl Handler<Wait> for ProgramActor {
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

impl Handler<Terminate> for ProgramActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _: Terminate, ctx: &mut Self::Context) -> Self::Result {
        if let Some(stop_tx) = self.stop_tx.take() {
            // The child is still running, wait for stopped_rx.
            stop_tx.send(()).expect("send stop signal");
        }
        Box::pin(ctx.address().send(Wait).map(|_| ()))
    }
}

/// Keep the program running until retry limits reached or terminate signal received.
#[derive(Message)]
#[rtype("()")]
pub struct Daemon(oneshot::Receiver<()>);

impl Handler<Daemon> for ProgramActor {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Daemon, ctx: &mut Self::Context) -> Self::Result {
        let stop_rx = msg.0;
        let addr = ctx.address();
        let one_round_fut = move |stop_rx: Receiver<()>| {
            wrap_future::<_, Self>(addr.send(Run(stop_rx))).map(|res, act, _| {
                let res = res.expect("self is alive");
                match res {
                    Ok(Some(stop_rx)) => {
                        act.logger
                            .do_send(Custom::new(act.name.clone(), "Exits without error."));
                        Some(stop_rx)
                    }
                    Ok(None) => {
                        // terminate signal received
                        act.logger.do_send(Custom::new(
                            act.name.clone(),
                            String::from("Terminated by exit signal."),
                        ));
                        None
                    }
                    Err((stop_rx, e)) => {
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
            Duration::from_secs(10), // TODO make it configurable
            self.program.retries.unwrap_or(usize::MAX),
        );
        RepeatedActFut::new(one_round_fut, stop_rx, retry_guard)
            .map(|res, act, _| {
                if !res {
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

impl Actor for ProgramActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (stopped_tx, _) = broadcast::channel(10);
        self.stopped_tx = Some(stopped_tx);
        self.stop_tx = Some(stop_tx);
        ctx.notify(Daemon(stop_rx));
    }
}
