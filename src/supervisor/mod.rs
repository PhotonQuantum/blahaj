use actix::{Actor, Addr, Context, Handler, MailboxError, Message, ResponseFuture};
use awc::Client;
use futures::{future, FutureExt};

use caretaker::CaretakerActor;
pub use caretaker::{HealthState, Lifecycle};

use crate::config::Program;
use crate::logger::LoggerActor;
use crate::supervisor::caretaker::{GetStatus, Terminate, Wait};

mod caretaker;
mod futs;
mod health;
mod retry;
mod terminate_ext;

pub struct Supervisor {
    actors: Vec<(String, Addr<CaretakerActor>)>,
}

impl Actor for Supervisor {
    type Context = Context<Self>;
}

#[derive(Debug, Message)]
#[rtype(result = "()")]
pub struct Join;

impl Handler<Join> for Supervisor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, _: Join, _: &mut Self::Context) -> Self::Result {
        future::try_join_all(self.actors.iter().map(|(_, addr)| addr.send(Wait)))
            .map(|_| ())
            .boxed_local()
    }
}

#[derive(Debug, Message)]
#[rtype(result = "()")]
pub struct TerminateAll;

impl Handler<TerminateAll> for Supervisor {
    type Result = ();

    fn handle(&mut self, _: TerminateAll, _: &mut Self::Context) -> Self::Result {
        self.actors
            .iter()
            .for_each(|(_, addr)| addr.do_send(Terminate));
    }
}

#[derive(Debug, Message)]
#[rtype(result = "Result<Vec<(String, Lifecycle)>, MailboxError>")]
pub struct GetStatusAll;

impl Handler<GetStatusAll> for Supervisor {
    type Result = ResponseFuture<Result<Vec<(String, Lifecycle)>, MailboxError>>;

    fn handle(&mut self, _: GetStatusAll, _: &mut Self::Context) -> Self::Result {
        future::try_join_all(self.actors.iter().map(|(name, addr)| {
            let name = name.clone();
            addr.send(GetStatus)
                .map(move |res| res.map(|lifecycle| (name, lifecycle)))
        }))
        .boxed_local()
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
                    (
                        name.clone(),
                        CaretakerActor::new(name, program, client.clone(), logger.clone()).start(),
                    )
                })
                .collect(),
        }
    }
}
