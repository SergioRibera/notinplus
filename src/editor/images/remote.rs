//! Images a note links from the web.
//!
//! A remote image is fetched only after the person agrees, once or for
//! every image of that host: a note that came from someone else (an
//! import, a shared document) must not tell a server when and from where
//! it was opened. Downloads never reach this device, the local network
//! or other private addresses, also when a redirect or a DNS answer
//! points there.

use std::collections::HashSet;
use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs as _};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::{ImageError, MAX_BYTES};
use crate::persist::Persisted;

/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(20);

/// Hosts whose images load without asking.
#[istmo::message]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TrustedHosts {
    hosts: Vec<String>,
}

impl Persisted for TrustedHosts {
    const FILE: &'static str = "remote_images.bin";
}

/// What the person agreed to load.
#[derive(Debug, Default)]
struct Consent {
    /// Read from disk on first use.
    hosts: Option<HashSet<String>>,
    /// Allowed once, until the app quits.
    urls: HashSet<String>,
}

static CONSENT: LazyLock<Mutex<Consent>> = LazyLock::new(Mutex::default);

fn consent() -> MutexGuard<'static, Consent> {
    CONSENT.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Consent {
    fn hosts(&mut self) -> &mut HashSet<String> {
        self.hosts
            .get_or_insert_with(|| TrustedHosts::load().hosts.into_iter().collect())
    }
}

/// Host of `url`, lowercase, for asking and remembering.
#[must_use]
pub(crate) fn host(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// Whether `url` may load without asking.
#[must_use]
pub(crate) fn allowed(url: &str) -> bool {
    let mut consent = consent();
    consent.urls.contains(url) || host(url).is_some_and(|host| consent.hosts().contains(&host))
}

/// Load `url` from now until the app quits.
pub(crate) fn allow_once(url: &str) {
    consent().urls.insert(url.to_owned());
}

/// Load every image of `url`'s host from now on, also after a restart.
pub(crate) fn allow_host(url: &str) {
    let Some(host) = host(url) else {
        return;
    };
    let mut consent = consent();
    if !consent.hosts().insert(host) {
        return;
    }
    let mut hosts: Vec<String> = consent.hosts().iter().cloned().collect();
    drop(consent);
    hosts.sort();
    if let Err(err) = (TrustedHosts { hosts }).save() {
        tracing::warn!("saving trusted image hosts: {err}");
    }
}

/// Download at most [`MAX_BYTES`] + 1 of `url`. Blocking.
///
/// # Errors
/// A private address, a refused redirect, a network failure or an HTTP
/// error status.
pub(super) fn download(url: &str) -> Result<Vec<u8>, ImageError> {
    let parsed = Url::parse(url).map_err(|_| ImageError::Unsupported)?;
    if !public_url(&parsed) {
        return Err(ImageError::Unsupported);
    }
    let response = client()?
        .get(parsed)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)?;
    let mut raw = Vec::new();
    response.take(MAX_BYTES + 1).read_to_end(&mut raw)?;
    Ok(raw)
}

fn client() -> Result<&'static reqwest::blocking::Client, ImageError> {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .dns_resolver(Arc::new(PublicOnly))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if public_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// An `http(s)` URL that does not name a private address outright. Host
/// names are checked when they resolve ([`PublicOnly`]).
fn public_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // IPv6 hosts come bracketed.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().map_or_else(
        |_| {
            let domain = bare.trim_end_matches('.').to_ascii_lowercase();
            domain != "localhost" && !domain.ends_with(".localhost")
        },
        is_public,
    )
}

/// Resolves host names to public addresses only, so a DNS answer cannot
/// point a download at the local network.
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let public: Vec<SocketAddr> = (host.as_str(), 0)
                .to_socket_addrs()?
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if public.is_empty() {
                return Err("no public address".into());
            }
            let addrs: Addrs = Box::new(public.into_iter());
            Ok(addrs)
        })
    }
}

/// Whether `ip` is reachable on the public internet.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map_or_else(|| is_public_v6(ip), is_public_v4),
    }
}

const fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        // Shared address space (carrier-grade NAT), 100.64.0.0/10.
        || (a == 100 && (b & 0xC0) == 64)
        // "This network", 0.0.0.0/8.
        || a == 0)
}

const fn is_public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        // Unique local, fc00::/7.
        || (first & 0xFE00) == 0xFC00
        // Link local, fe80::/10.
        || (first & 0xFFC0) == 0xFE80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses_are_refused() {
        for url in [
            "http://127.0.0.1/a.png",
            "http://192.168.1.1/a.png",
            "http://10.0.0.5/a.png",
            "http://100.64.0.1/a.png",
            "http://169.254.169.254/latest",
            "http://[::1]/a.png",
            "http://[fd00::1]/a.png",
            "http://[fe80::1]/a.png",
            "http://[::ffff:192.168.0.1]/a.png",
            "http://localhost/a.png",
            "http://printer.localhost/a.png",
            "file:///etc/passwd",
            "ftp://example.com/a.png",
        ] {
            assert!(!public_url(&Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "https://example.com/a.png",
            "http://93.184.216.34/a.png",
            "https://[2606:2800:220:1::]/a.png",
        ] {
            assert!(public_url(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[test]
    fn hosts_are_compared_lowercase() {
        assert_eq!(
            host("https://Example.COM/x.png").as_deref(),
            Some("example.com")
        );
        assert_eq!(host("not a url"), None);
    }
}
