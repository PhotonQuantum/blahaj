#![allow(clippy::enum_variant_names)]
mod config;
mod error;
mod supervisor;
mod logger;

#[actix_web::main]
async fn main() {
    println!("Hello, world!");
}