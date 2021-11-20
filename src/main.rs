#![allow(clippy::enum_variant_names)]

use std::fs::File;

use actix::{Actor, Addr};

use crate::config::Config;
use crate::ctrlc_handler::SignalHandler;
use crate::logger::LoggerActor;
use crate::supervisor::{Join, Supervisor};

mod config;
mod ctrlc_handler;
mod error;
mod logger;
mod supervisor;

#[actix::main]
async fn main() {
    tracing_subscriber::fmt().init();
    let f = File::open("config.yaml").unwrap();
    let config: Config = serde_yaml::from_reader(f).expect("config");
    let logger = LoggerActor.start();
    let supervisor = Supervisor::start(config.programs, logger);
    let supervisor_addr: Addr<Supervisor> = supervisor.start();
    let handler = SignalHandler::new(supervisor_addr.clone());
    let _handler_addr = handler.start();
    supervisor_addr.send(Join).await.expect("join");
    println!("joined");
}
