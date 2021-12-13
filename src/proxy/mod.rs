use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use awc::http::uri::{Authority, PathAndQuery, Scheme};
use awc::http::Uri;
use log::info;

use crate::config::HttpRelay;

mod forwarded_header;
pub mod handler;

/// A proxy upstream.
#[derive(Debug)]
struct ProxyEntry {
    port: u16,
    secure: bool,
    prefix_path: String, // NOTE this path must have its prefix and suffix slash trimmed.
}

impl From<&HttpRelay> for ProxyEntry {
    fn from(relay: &HttpRelay) -> Self {
        Self {
            port: relay.port,
            secure: relay.https,
            prefix_path: if relay.strip_path {
                String::from("")
            } else {
                relay.path.clone()
            },
        }
    }
}

pub struct ProxyConfig {
    /// A map that projects requested path to upstream entry.
    ///
    /// NOTE the key must have its prefix and suffix slash trimmed.
    map: HashMap<PathBuf, ProxyEntry>,
    interface: IpAddr,
}

impl ProxyConfig {
    pub fn new<'a>(i: impl IntoIterator<Item = &'a HttpRelay>, bind: SocketAddr) -> Self {
        Self {
            map: i
                .into_iter()
                .map(|entry: &HttpRelay| {
                    (
                        PathBuf::from(entry.path.trim_matches('/')),
                        ProxyEntry::from(entry),
                    )
                })
                .collect(),
            interface: bind.ip(),
        }
    }
}

impl ProxyConfig {
    pub fn get(&self, incoming_req: &Uri) -> Option<Uri> {
        info!("incoming req: {}", incoming_req);
        let incoming_path = Path::new(incoming_req.path().trim_start_matches('/'));
        let (matched, entry) = incoming_path
            .ancestors()
            .find_map(|path| self.map.get(path).map(|entry| (path, entry)))?;
        info!("matched entry: {:?}", entry);

        let scheme = if entry.secure {
            Scheme::HTTPS
        } else {
            Scheme::HTTP
        };

        let path = format!(
            "{}/{}",
            entry.prefix_path,
            incoming_req
                .path()
                .strip_prefix(&format!("/{}", matched.to_str()?))?
        );
        info!("new path: {}", path);
        let path_query = if let Some(query) = incoming_req.query() {
            PathAndQuery::try_from(format!("{}?{}", path, query))
        } else {
            PathAndQuery::try_from(path)
        }
        .ok()?;
        info!("new_path_query: {}", path_query);

        Some(
            Uri::builder()
                .scheme(scheme)
                .authority(Authority::try_from(format!("127.0.0.1:{}", entry.port)).ok()?)
                .path_and_query(path_query)
                .build()
                .expect("build new uri"),
        )
    }
}
