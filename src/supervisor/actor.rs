use std::collections::VecDeque;
use std::env;
use std::future::Future;
use std::mem::ManuallyDrop;
use std::ops::DerefMut;
use std::pin::Pin;
use std::process::Stdio;
use std::task::Poll;
use std::time::{Duration, Instant};

use actix::fut::{ready, wrap_future};
use actix::{
    Actor, ActorFuture, ActorFutureExt, Addr, AsyncContext, Context, Handler, Message,
    ResponseActFuture, ResponseFuture,
};
use futures::{ready, FutureExt, Stream, StreamExt};
use tokio::io::AsyncRead;
use tokio::process::{Child, Command};
use tokio::sync::oneshot::Receiver;
use tokio::sync::{broadcast, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::config::{Env, Program};
use crate::error::Error;
use crate::logger::{ChildOutput, IOType, LoggerActor, RegisterStdio};
use crate::supervisor::terminate_ext::TerminateExt;

#[derive(Debug)]
struct RetryGuard {
    retries: VecDeque<Instant>,
    tolerance_interval: Duration,
    tolerance_count: usize,
}

impl RetryGuard {
    /// Evict all events before a given time.
    fn evict(&mut self, before: Instant) {
        let evict_range = 0..self.retries.iter().filter(|item| *item < &before).count();
        drop(self.retries.drain(evict_range));
    }

    /// Mark current event.
    /// Returns `false` if the upperbound is exceeded.
    fn mark(&mut self) -> bool {
        let now = Instant::now();
        self.evict(now - self.tolerance_interval);
        self.retries.len() <= self.tolerance_count
    }
}

#[derive(Debug)]
pub struct ProgramActor {
    name: String,
    program: Program,
    stop_tx: Option<oneshot::Sender<()>>,
    stopped_tx: Option<broadcast::Sender<()>>,
    logger: Addr<LoggerActor>,
}

impl ProgramActor {
    pub fn new(name: String, program: Program, logger: Addr<LoggerActor>) -> Self {
        ProgramActor {
            name,
            program,
            stop_tx: None,
            stopped_tx: None,
            logger,
        }
    }
}

fn build_child(program: &Program) -> Result<Child, Error> {
    let cmd = &program.command;
    let mut command = Command::new(&cmd.cmd);
    command
        .args(&cmd.args)
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
struct Run(oneshot::Receiver<()>);

impl Handler<Run> for ProgramActor {
    type Result =
        ResponseFuture<Result<Option<oneshot::Receiver<()>>, (oneshot::Receiver<()>, Error)>>;

    fn handle(&mut self, msg: Run, _: &mut Self::Context) -> Self::Result {
        let stop_rx = msg.0;
        let program = self.program.clone();

        struct WaitAbortFut {
            child: Option<Child>,
            stop_rx: Option<oneshot::Receiver<()>>,
        }
        impl WaitAbortFut {
            pub fn new(child: Child, stop_rx: oneshot::Receiver<()>) -> Self {
                WaitAbortFut {
                    child: Some(child),
                    stop_rx: Some(stop_rx),
                }
            }
        }

        impl Future for WaitAbortFut {
            type Output =
                Result<Result<oneshot::Receiver<()>, Child>, (oneshot::Receiver<()>, Error)>;

            fn poll(
                mut self: Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> Poll<Self::Output> {
                let mut child = self.child.take().expect("polled twice");
                let mut stop_rx = self.stop_rx.take().expect("polled twice");
                match stop_rx.poll_unpin(cx) {
                    Poll::Ready(_) => Poll::Ready(Ok(Err(child))),
                    Poll::Pending => {
                        // SAFETY we know that child.wait() doesn't have drop side effect.
                        let mut wait_fut = ManuallyDrop::new(child.wait());
                        let wait_fut_ref = unsafe { Pin::new_unchecked(wait_fut.deref_mut()) };
                        match wait_fut_ref.poll(cx) {
                            Poll::Ready(res) => Poll::Ready(if let Ok(res) = res {
                                if !res.success() {
                                    Err((stop_rx, Error::NonZeroExitError(res.code().unwrap_or(0))))
                                } else {
                                    Ok(Ok(stop_rx))
                                }
                            } else {
                                Err((stop_rx, Error::AlreadyDiedError))
                            }),
                            Poll::Pending => {
                                self.child = Some(child);
                                self.stop_rx = Some(stop_rx);
                                Poll::Pending
                            }
                        }
                    }
                }
            }
        }

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
        let logger = self.logger.clone();
        let name = self.name.clone();
        Box::pin(async move {
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
                    logger
                        .send(RegisterStdio(stdout))
                        .await
                        .expect("register stdout logger");
                    logger
                        .send(RegisterStdio(stderr))
                        .await
                        .expect("register stderr logger");

                    let res = WaitAbortFut::new(child, stop_rx).await;
                    match res {
                        Ok(Err(mut child)) => {
                            if tokio::time::timeout(Duration::from_secs(5), child.terminate())
                                .await
                                .is_err()
                            {
                                drop(child.kill().await);
                            }
                            Ok(None)
                        }
                        Ok(Ok(stop_rx)) => Ok(Some(stop_rx)),
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err((stop_rx, e)),
            }
        })
    }
}

/// Wait for the child to exit.
#[derive(Message)]
#[rtype("()")]
pub struct Wait;

impl Handler<Wait> for ProgramActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _: Wait, _: &mut Self::Context) -> Self::Result {
        if let Some(stopped_tx) = &self.stopped_tx {
            // The child is terminating, wait for stopped_rx.
            let mut stopped_rx = stopped_tx.subscribe();
            Box::pin(async move {
                drop(stopped_rx.recv().await);
            })
        } else {
            // The child is terminated.
            Box::pin(ready(()))
        }
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
struct Daemon(oneshot::Receiver<()>);

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
                        // TODO log success exit
                        Some(stop_rx)
                    }
                    Ok(None) => {
                        // terminate signal received
                        act.stopped_tx
                            .take()
                            .unwrap()
                            .send(())
                            .expect("send stopped_tx signal");
                        None
                    }
                    Err((stop_rx, _e)) => {
                        // TODO log failure exit
                        Some(stop_rx)
                    }
                }
            })
        };
        struct RepeatedActFut<F, Fut, I> {
            factory: F,
            fut: Option<Fut>,
            state: Option<I>,
            retry_guard: RetryGuard,
        }
        impl<F, Fut, I> RepeatedActFut<F, Fut, I> {
            pub fn new(factory: F, initial_state: I, retry_guard: RetryGuard) -> Self {
                RepeatedActFut {
                    factory,
                    fut: None,
                    state: Some(initial_state),
                    retry_guard,
                }
            }
        }
        impl<A, Fut, F, I> ActorFuture<A> for RepeatedActFut<F, Fut, I>
        where
            A: Actor,
            Fut: ActorFuture<A, Output = Option<I>>,
            F: Fn(I) -> Fut + Unpin,
            I: Unpin,
        {
            type Output = ();

            fn poll(
                mut self: Pin<&mut Self>,
                srv: &mut A,
                ctx: &mut A::Context,
                task: &mut std::task::Context<'_>,
            ) -> Poll<Self::Output> {
                // SAFETY we wrap fut into Pin again immediately after verify Option's variant
                if let Some(fut) = &mut unsafe { Pin::get_unchecked_mut(self.as_mut()) }.fut {
                    let fut = unsafe { Pin::new_unchecked(fut) };
                    match ready!(fut.poll(srv, ctx, task)) {
                        Some(res) => {
                            // Fut finished but need to be recreated, this means that process exited
                            // SAFETY RetryGuard is Unpin
                            if unsafe { Pin::get_unchecked_mut(self.as_mut()) }
                                .retry_guard
                                .mark()
                            {
                                // SAFETY state is Unpin, and fut is dropped
                                let this = &mut unsafe { Pin::get_unchecked_mut(self.as_mut()) };
                                this.state = Some(res);
                                this.fut = None;
                                Poll::Pending
                            } else {
                                // retry limit exceeds, bail out
                                Poll::Ready(())
                            }
                        }
                        None => {
                            // Fut fully resolves
                            Poll::Ready(())
                        }
                    }
                } else {
                    // SAFETY state is Unpin
                    let state = unsafe { Pin::get_unchecked_mut(self.as_mut()) }
                        .state
                        .take()
                        .expect("no state available");
                    // No fut created, create from factory
                    let fut = (self.factory)(state);
                    // SAFETY we put fut into its slot
                    unsafe { Pin::get_unchecked_mut(self.as_mut()) }.fut = Some(fut);
                    Poll::Pending
                }
            }
        }
        let retry_guard = RetryGuard {
            retries: VecDeque::new(),
            tolerance_interval: Duration::from_secs(10), // TODO make it configurable
            tolerance_count: self.program.retries.unwrap_or(usize::MAX),
        };
        Box::pin(RepeatedActFut::new(one_round_fut, stop_rx, retry_guard))
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
