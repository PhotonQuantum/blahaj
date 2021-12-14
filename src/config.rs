use std::collections::HashMap;
use std::env::VarError;
use std::fmt::Formatter;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use serde::de;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use serde_with_expand_env::with_expand_envs;
use shellexpand::LookupError;
use shlex::Shlex;

use crate::error::SupervisorError;

fn default_scope() -> String {
    String::from("/blahaj")
}

#[derive(Debug, Eq, PartialEq, Deserialize)]
pub struct Config {
    #[serde(deserialize_with = "with_expand_envs")]
    pub bind: SocketAddr,
    #[serde(default = "default_scope", deserialize_with = "with_expand_envs")]
    pub api_scope: String,
    pub programs: HashMap<String, Program>,
}

const fn default_stop_grace() -> Duration {
    Duration::from_secs(10)
}

#[derive(Debug, Eq, PartialEq, Deserialize, Clone)]
pub struct Program {
    pub command: CommandLine,
    #[serde(alias = "environment", default)]
    pub env: HashMap<String, Env>,
    pub http: Option<HttpRelay>,
    #[serde(default)]
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

#[derive(Debug, Copy, Clone, Eq, PartialEq, Deserialize)]
pub struct Retry {
    #[serde(with = "humantime_serde", default = "default_retry_window")]
    pub window: Duration,
    #[serde(default = "default_retry_count", deserialize_with = "with_expand_envs")]
    pub count: usize,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            window: default_retry_window(),
            count: default_retry_count(),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct HttpRelay {
    #[serde(deserialize_with = "with_expand_envs")]
    pub port: u16,
    #[serde(deserialize_with = "with_expand_envs")]
    pub path: String,
    #[serde(default, deserialize_with = "with_expand_envs")]
    pub https: bool,
    #[serde(default, deserialize_with = "with_expand_envs")]
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

#[derive(Debug, Eq, PartialEq, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct HealthCheck {
    #[serde(deserialize_with = "with_expand_envs")]
    pub path: String,
    #[serde(with = "humantime_serde", default = "default_health_interval")]
    pub interval: Duration,
    #[serde(with = "humantime_serde", default = "default_health_grace")]
    pub grace: Duration,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Env(pub Option<String>);

impl FromStr for Env {
    type Err = LookupError<VarError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Some(shellexpand::env(s)?.to_string())))
    }
}

struct EnvVisitor;

impl<'de> Visitor<'de> for EnvVisitor {
    type Value = Env;

    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("an environment value or null")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> where E: de::Error {
        Env::from_str(v).map_err(E::custom)
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> where E: de::Error {
        Env::from_str(v).map_err(E::custom)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> where E: de::Error {
        Env::from_str(v.as_str()).map_err(E::custom)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> where E: de::Error {
        Ok(Env(None))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error> where D: Deserializer<'de> {
        deserializer.deserialize_string(self)
    }
}

impl<'de> Deserialize<'de> for Env {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        deserializer.deserialize_option(EnvVisitor)
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
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
        let v = shellexpand::env(v).map_err(E::custom)?;
        CommandLine::from_str(&*v).map_err(E::custom)
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let v = shellexpand::env(v).map_err(E::custom)?;
        CommandLine::from_str(&*v).map_err(E::custom)
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let v = shellexpand::env(&v).map_err(E::custom)?;
        CommandLine::from_str(&*v).map_err(E::custom)
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
