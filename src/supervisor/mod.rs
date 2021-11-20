use actix::{Actor, Addr};
use futures::future;

use actor::ProgramActor;

use crate::config::Program;
use crate::logger::LoggerActor;
use crate::supervisor::actor::Wait;

mod actor;
mod terminate_ext;

#[derive(Debug)]
pub struct Supervisor {
    actors: Vec<Addr<ProgramActor>>,
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
    pub async fn join(self) {
        future::try_join_all(self.actors.into_iter().map(|addr| addr.send(Wait)))
            .await
            .expect("join");
    }
}
