#![allow(clippy::enum_variant_names)]
mod config;
mod error;
mod logger;
mod supervisor;

#[actix_web::main]
async fn main() {
    println!("Hello, world!");
}
