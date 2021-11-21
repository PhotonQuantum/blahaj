use std::future::{ready, Future};
use std::pin::Pin;

use nix::libc::pid_t;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tokio::process::Child;

type Output<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

pub trait TerminateExt {
    fn terminate(&mut self) -> Output;
}

impl TerminateExt for Child {
    //noinspection RsUnresolvedReference
    fn terminate(&mut self) -> Output {
        self.id().map_or_else(
            || Box::pin(ready(())) as Output,
            |pid| {
                #[allow(clippy::cast_possible_wrap)]
                let _ = signal::kill(Pid::from_raw(pid as pid_t), Signal::SIGTERM);
                Box::pin(async {
                    drop(self.wait().await);
                })
            },
        )
    }
}
