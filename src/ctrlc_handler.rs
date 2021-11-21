use actix::{Actor, Addr, AsyncContext, Context, Running, StreamHandler};
use nix::libc::c_int;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook::iterator::Handle;
use signal_hook_tokio::Signals;

use crate::logger::Custom;
use crate::supervisor::TerminateAll;
use crate::{LoggerActor, Supervisor};

pub struct SignalHandler {
    supervisor: Addr<Supervisor>,
    handle: Option<Handle>,
    logger: Addr<LoggerActor>,
}

impl SignalHandler {
    pub const fn new(supervisor: Addr<Supervisor>, logger: Addr<LoggerActor>) -> Self {
        Self {
            supervisor,
            handle: None,
            logger,
        }
    }
}

impl StreamHandler<c_int> for SignalHandler {
    fn handle(&mut self, _: c_int, _: &mut Self::Context) {
        self.logger
            .do_send(Custom::new("signal", "Terminating all processes"));
        self.supervisor.do_send(TerminateAll);
    }
}

impl Actor for SignalHandler {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let signals = Signals::new(TERM_SIGNALS).expect("signal handler");
        self.handle = Some(signals.handle());
        ctx.add_stream(signals);
    }

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        if let Some(handle) = self.handle.take() {
            handle.close();
        }
        Running::Stop
    }
}
