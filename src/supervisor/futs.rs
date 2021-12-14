use std::future::Future;
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::pin::Pin;
use std::task::Poll;

use actix::{Actor, ActorFuture};
use futures::{ready, FutureExt};
use signal_hook::low_level::signal_name;
use tokio::process::Child;
use tokio::sync::oneshot::Receiver;

use crate::error::SupervisorError;

use super::retry::RetryGuard;

/// Repeatedly poll a given future with a given initial state until it returns None state or
/// retry guard fails.
/// Resolves to `fail` if retry guard fails.
pub struct RepeatedActFut<F, Fut, I> {
    factory: F,
    fut: Option<Fut>,
    state: Option<I>,
    retry_guard: RetryGuard,
}

impl<F, Fut, I> RepeatedActFut<F, Fut, I> {
    pub const fn new(factory: F, initial_state: I, retry_guard: RetryGuard) -> Self {
        Self {
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
    type Output = bool;

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
                        let this = unsafe { Pin::get_unchecked_mut(self.as_mut()) };
                        this.state = Some(res);
                        this.fut = None;
                        task.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        // retry limit exceeds, bail out
                        Poll::Ready(false)
                    }
                }
                None => {
                    // Fut fully resolves due to termination signal
                    Poll::Ready(true)
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
            task.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

pub struct WaitAbortFut {
    child: Option<Child>,
    stop_rx: Option<Receiver<()>>,
    unhealthy_rx: Option<Receiver<()>>,
}

impl WaitAbortFut {
    pub fn new(child: Child, stop_rx: Receiver<()>, unhealthy_rx: Receiver<()>) -> Self {
        Self {
            child: Some(child),
            stop_rx: Some(stop_rx),
            unhealthy_rx: Some(unhealthy_rx),
        }
    }
}

/// Return value of `WaitAbortFut`.
pub enum WaitAbortResult {
    /// Unhealthy signal received. Return signal handle and child handle.
    UnhealthySignalReceived(Receiver<()>, Child),
    /// Stop signal received. Return child handle.
    StopSignalReceived(Child),
    /// Process exited without error. Return signal handle.
    Exit(Receiver<()>),
    /// Process exited with error. Return signal handle and error.
    ExitWithError((Receiver<()>, SupervisorError)),
}

impl Future for WaitAbortFut {
    type Output = WaitAbortResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        // These unwraps shouldn't fail because we either initialized them or we put them back before returning `Pending`.
        let mut child = self.child.take().unwrap();
        let mut stop_rx = self.stop_rx.take().unwrap();
        let mut unhealthy_rx = self.unhealthy_rx.take().unwrap();
        match (stop_rx.poll_unpin(cx), unhealthy_rx.poll_unpin(cx)) {
            (Poll::Ready(_), _) => Poll::Ready(WaitAbortResult::StopSignalReceived(child)),
            (_, Poll::Ready(_)) => {
                Poll::Ready(WaitAbortResult::UnhealthySignalReceived(stop_rx, child))
            }
            (Poll::Pending, Poll::Pending) => {
                // SAFETY we know that child.wait() doesn't have drop side effect.
                let mut wait_fut = ManuallyDrop::new(child.wait());
                let wait_fut_ref = unsafe { Pin::new_unchecked(&mut *wait_fut) };
                match wait_fut_ref.poll(cx) {
                    Poll::Ready(res) => Poll::Ready(if let Ok(res) = res {
                        if res.success() {
                            WaitAbortResult::Exit(stop_rx)
                        } else {
                            let exit_code = res.code();
                            let err = if let Some(exit_code) = exit_code {
                                SupervisorError::NonZeroExit(exit_code)
                            } else if cfg!(unix) {
                                SupervisorError::ExitBySignal(
                                    signal_name(res.signal().expect("exit signal"))
                                        .unwrap_or("UNKNOWN"),
                                )
                            } else {
                                SupervisorError::NonZeroExit(-1)
                            };
                            WaitAbortResult::ExitWithError((stop_rx, err))
                        }
                    } else {
                        WaitAbortResult::ExitWithError((stop_rx, SupervisorError::AlreadyDied))
                    }),
                    Poll::Pending => {
                        self.child = Some(child);
                        self.stop_rx = Some(stop_rx);
                        self.unhealthy_rx = Some(unhealthy_rx);
                        Poll::Pending
                    }
                }
            }
        }
    }
}
