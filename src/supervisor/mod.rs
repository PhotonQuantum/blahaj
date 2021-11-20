use actix::{Actor, Addr, Context, Handler, Message, ResponseFuture, Running};
use futures::{future, FutureExt};

use actor::ProgramActor;

use crate::config::Program;
use crate::logger::LoggerActor;
use crate::supervisor::actor::{Terminate, Wait};

mod actor;
mod terminate_ext;

#[derive(Debug)]
pub struct Supervisor {
    actors: Vec<Addr<ProgramActor>>,
}

impl Actor for Supervisor {
    type Context = Context<Self>;
}

#[derive(Debug, Message)]
#[rtype("()")]
pub struct Join;

impl Handler<Join> for Supervisor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _: Join, _: &mut Self::Context) -> Self::Result {
        Box::pin(future::try_join_all(self.actors.iter().map(|addr| addr.send(Wait))).map(|_| ()))
    }
}

#[derive(Debug, Message)]
#[rtype("()")]
pub struct TerminateAll;

impl Handler<TerminateAll> for Supervisor {
    type Result = ();

    fn handle(&mut self, _: TerminateAll, _: &mut Self::Context) -> Self::Result {
        self.actors.iter().for_each(|addr| addr.do_send(Terminate))
    }
}

impl Supervisor {
    pub fn start(
        programs: impl IntoIterator<Item = (String, Program)>,
        logger: Addr<LoggerActor>,
    ) -> Self {
        Self {
            actors: programs
                .into_iter()
                .map(|(name, program)| ProgramActor::new(name, program, logger.clone()).start())
                .collect(),
        }
    }
}
