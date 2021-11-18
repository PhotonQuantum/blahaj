use std::collections::HashMap;
use std::fmt::Formatter;
use std::net::SocketAddr;
use std::str::FromStr;
use serde::de::Visitor;
use serde::de;
use serde::{Deserialize, Deserializer};
use shlex::Shlex;
use crate::error::Error;
use crate::error::Error::CommandError;

#[derive(Deserialize)]
pub struct Config {
    bind: SocketAddr,
    programs: HashMap<String, Program>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Program {
    pub command: CommandLine,
    #[serde(alias="environment")]
    pub env: HashMap<String, Option<Env>>,
    pub http: HttpRelay,
    pub retries: Option<usize>
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct HttpRelay {
    port: u16,
    path: String,
    health_check: String
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    Literal(String),
    Inherit(String)
}

#[derive(Debug, Clone)]
pub struct CommandLine {
    pub cmd: String,
    pub args: Vec<String>
}

impl FromStr for CommandLine {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut lexer = Shlex::new(s);
        let cmd = lexer.next().ok_or(CommandError)?;
        let args = lexer.collect();
        Ok(CommandLine {
            cmd,
            args
        })
    }
}

struct CommandLineVisitor;

impl<'de> Visitor<'de> for CommandLineVisitor {
    type Value = CommandLine;

    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("a command line string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> where E: de::Error {
        CommandLine::from_str(v).map_err(E::custom)
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> where E: de::Error {
        CommandLine::from_str(v).map_err(E::custom)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> where E: de::Error {
        CommandLine::from_str(v.as_str()).map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for CommandLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        deserializer.deserialize_string(CommandLineVisitor)
    }
}