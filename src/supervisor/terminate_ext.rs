use std::future::{Future, ready};
use std::pin::Pin;
use nix::libc::pid_t;
use tokio::process::Child;
use nix::unistd::Pid;
use nix::sys::signal::{self, Signal};

pub trait TerminateExt {
    fn terminate<'a>(&'a mut self) -> Pin<Box<dyn Future<Output=()> + 'a>>;
}

impl TerminateExt for Child {
    fn terminate<'a>(&'a mut self) -> Pin<Box<dyn Future<Output=()> + 'a>> {
        if let Some(pid) = self.id() {
            let _ = signal::kill(Pid::from_raw(pid as pid_t), Signal::SIGTERM);
            Box::pin(async {
                drop(self.wait().await);
            })
        } else {
            Box::pin(ready(()))
        }
    }
}