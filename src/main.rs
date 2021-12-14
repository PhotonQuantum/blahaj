#![allow(
    clippy::enum_variant_names,
    clippy::module_name_repetitions,
    clippy::future_not_send,
    clippy::default_trait_access
)]

use std::fs::File;

use actix::{Actor, Addr};
use actix_web::middleware::Logger;
use actix_web::web::Data;
use actix_web::{web, App, HttpServer};
use awc::Client;
use log::info;

use crate::config::Config;
use crate::ctrlc_handler::SignalHandler;
use crate::endpoints::{health, status};
use crate::logger::LoggerActor;
use crate::proxy::ProxyConfig;
use crate::supervisor::{Join, Supervisor};

mod config;
mod ctrlc_handler;
mod endpoints;
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

    let client = Client::new();
    let supervisor = Supervisor::start(config.programs.clone(), &client, &logger);
    let supervisor_addr: Addr<Supervisor> = supervisor.start();
    let supervisor_addr_data = Data::new(supervisor_addr.clone());

    let signal_handler = SignalHandler::new(supervisor_addr.clone(), logger);
    let _signal_handler_addr = signal_handler.start();

    let proxy_config = Data::new(ProxyConfig::new(
        config
            .programs
            .iter()
            .filter_map(|(_, program)| program.http.as_ref()),
        config.bind,
    ));

    info!("Serving at {}", config.bind);
    let server = HttpServer::new(move || {
        let client = Client::new();
        App::new()
            .wrap(Logger::default())
            .app_data(Data::new(client))
            .app_data(proxy_config.clone())
            .app_data(supervisor_addr_data.clone())
            .service(
                web::scope(config.api_scope.as_str())
                    .route("health", web::get().to(health))
                    .route("status", web::get().to(status)),
            )
            .default_service(web::to(proxy::handler::forward))
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
