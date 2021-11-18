use actix::{Actor, Addr};

use actor::ProgramActor;

use crate::config::Program;
use crate::logger::LoggerActor;

mod actor;
mod terminate_ext;

pub struct Supervisor {
    actors: Vec<Addr<ProgramActor>>,
}

impl Supervisor {
    pub fn start(programs: impl IntoIterator<Item=(String, Program)>, logger: Addr<LoggerActor>) -> Self {
        Self {
            actors: programs
                .into_iter()
                .map(|(name, program)| ProgramActor::new(name, program, logger.clone()).start())
                .collect(),
        }
    }
}
