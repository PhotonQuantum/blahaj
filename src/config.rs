use std::collections::HashMap;
use std::fmt::Formatter;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use serde::de;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use shlex::Shlex;

use crate::error::SupervisorError;

#[derive(Deserialize)]
pub struct Config {
    pub bind: SocketAddr,
    pub programs: HashMap<String, Program>,
}

const fn default_stop_grace() -> Duration {
    Duration::from_secs(10)
}

#[derive(Debug, Deserialize, Clone)]
pub struct Program {
    pub command: CommandLine,
    #[serde(alias = "environment", default)]
    pub env: HashMap<String, Option<Env>>,
    pub http: Option<HttpRelay>,
    pub retry: Retry,
    #[serde(with = "humantime_serde", default = "default_stop_grace")]
    pub grace: Duration,
}

const fn default_retry_window() -> Duration {
    Duration::from_secs(10)
}

const fn default_retry_count() -> usize {
    usize::MAX
}

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct Retry {
    #[serde(with = "humantime_serde", default = "default_retry_window")]
    pub window: Duration,
    #[serde(default = "default_retry_count")]
    pub count: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct HttpRelay {
    pub port: u16,
    pub path: String,
    #[serde(default)]
    pub https: bool,
    #[serde(default)]
    pub strip_path: bool,
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
}

const fn default_health_interval() -> Duration {
    Duration::from_secs(5)
}

const fn default_health_grace() -> Duration {
    Duration::from_secs(30)
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct HealthCheck {
    pub path: String,
    #[serde(with = "humantime_serde", default = "default_health_interval")]
    pub interval: Duration,
    #[serde(with = "humantime_serde", default = "default_health_grace")]
    pub grace: Duration,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    Literal(String),
    Inherit(String),
}

#[derive(Debug, Clone)]
pub struct CommandLine {
    pub cmd: String,
    pub args: Vec<String>,
}

impl FromStr for CommandLine {
    type Err = SupervisorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut lexer = Shlex::new(s);
        let cmd = lexer.next().ok_or(SupervisorError::Command)?;
        let args = lexer.collect();
        Ok(Self { cmd, args })
    }
}

struct CommandLineVisitor;

impl<'de> Visitor<'de> for CommandLineVisitor {
    type Value = CommandLine;

    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("a command line string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        CommandLine::from_str(v).map_err(E::custom)
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        CommandLine::from_str(v).map_err(E::custom)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        CommandLine::from_str(v.as_str()).map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for CommandLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(CommandLineVisitor)
    }
}
