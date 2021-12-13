#![allow(clippy::enum_variant_names)]
#![allow(clippy::module_name_repetitions)]

use std::fs::File;

use actix::{Actor, Addr};
use actix_web::middleware::Logger;
use actix_web::web::Data;
use actix_web::{web, App, HttpServer};
use awc::Client;

use crate::config::Config;
use crate::ctrlc_handler::SignalHandler;
use crate::logger::LoggerActor;
use crate::proxy::ProxyConfig;
use crate::supervisor::{Join, Supervisor};

mod config;
mod ctrlc_handler;
mod error;
mod logger;
mod proxy;
mod supervisor;

#[actix::main]
async fn main() -> std::io::Result<()> {
    pretty_env_logger::init();
    let f = File::open("config.yaml").unwrap();
    let config: Config = serde_yaml::from_reader(f).expect("config");
    let logger = LoggerActor.start();

    let supervisor = Supervisor::start(config.programs.clone(), &logger);
    let supervisor_addr: Addr<Supervisor> = supervisor.start();

    let signal_handler = SignalHandler::new(supervisor_addr.clone(), logger);
    let _signal_handler_addr = signal_handler.start();

    let proxy_config = Data::new(ProxyConfig::new(
        config
            .programs
            .iter()
            .filter_map(|(_, program)| program.http.as_ref()),
        config.bind,
    ));

    let server = HttpServer::new(move || {
        let client = Client::new();
        App::new()
            .wrap(Logger::default())
            .app_data(Data::new(client))
            .app_data(proxy_config.clone())
            .default_service(web::route().to(proxy::handler::forward))
    })
    .bind(config.bind)?
    .run();
    let server_handle = server.handle();

    actix::spawn(async move {
        supervisor_addr.send(Join).await.expect("join");
        server_handle.stop(true).await;
    });

    server.await?;
    Ok(())
}
