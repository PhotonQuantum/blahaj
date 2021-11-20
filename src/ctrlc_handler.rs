use actix::{Actor, Addr, AsyncContext, Context, Running, StreamHandler};
use nix::libc::c_int;
use signal_hook::consts::{SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::Handle;
use signal_hook_tokio::Signals;

use crate::supervisor::TerminateAll;
use crate::Supervisor;

pub struct SignalHandler {
    supervisor: Addr<Supervisor>,
    handle: Option<Handle>,
}

impl SignalHandler {
    pub fn new(supervisor: Addr<Supervisor>) -> Self {
        SignalHandler {
            supervisor,
            handle: None,
        }
    }
}

impl StreamHandler<c_int> for SignalHandler {
    fn handle(&mut self, item: c_int, _ctx: &mut Self::Context) {
        match item {
            SIGQUIT | SIGINT | SIGTERM => self.supervisor.do_send(TerminateAll),
            _ => unreachable!(),
        }
    }
}

impl Actor for SignalHandler {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let signals = Signals::new(&[SIGQUIT, SIGINT, SIGTERM]).expect("signal handler");
        self.handle = Some(signals.handle());
        ctx.add_stream(signals);
    }

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        if let Some(handle) = self.handle.take() {
            handle.close()
        }
        Running::Stop
    }
}
