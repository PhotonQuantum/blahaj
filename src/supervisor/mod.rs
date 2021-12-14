use actix::{Actor, Addr, Context, Handler, Message, ResponseFuture};
use awc::Client;
use futures::{future, FutureExt};

use caretaker::CaretakerActor;

use crate::config::Program;
use crate::logger::LoggerActor;
use crate::supervisor::caretaker::{Terminate, Wait};

mod caretaker;
mod futs;
mod health;
mod retry;
mod terminate_ext;

pub struct Supervisor {
    actors: Vec<Addr<CaretakerActor>>,
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
        self.actors.iter().for_each(|addr| addr.do_send(Terminate));
    }
}

impl Supervisor {
    pub fn start(
        programs: impl IntoIterator<Item = (String, Program)>,
        client: &Client,
        logger: &Addr<LoggerActor>,
    ) -> Self {
        Self {
            actors: programs
                .into_iter()
                .map(|(name, program)| {
                    CaretakerActor::new(name, program, client.clone(), logger.clone()).start()
                })
                .collect(),
        }
    }
}
