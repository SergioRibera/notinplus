//! `notein-relay --port 7720 --data ./relay-data [--name "Relay"]`
//!
//! Prints the address to put in invitations and keeps running until
//! interrupted. The keypair is created on first run under `--data`.

use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use notein_sync::{DeviceProfile, RelayAddr, RelayConfig, RelayServer, RelayStore};

const DEFAULT_PORT: u16 = 7720;

#[derive(Debug)]
struct Args {
    port: u16,
    data: PathBuf,
    name: String,
    /// Address to print for invitations; defaults to the first
    /// non-loopback IPv4 of this host.
    advertise: Option<String>,
}

#[derive(Debug)]
enum ArgError {
    Unknown(String),
    Missing(&'static str),
    BadPort(String),
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(flag) => write!(f, "unknown argument {flag}"),
            Self::Missing(flag) => write!(f, "{flag} needs a value"),
            Self::BadPort(text) => write!(f, "bad port {text}"),
        }
    }
}

impl std::error::Error for ArgError {}

impl Args {
    fn parse(mut raw: impl Iterator<Item = String>) -> Result<Self, ArgError> {
        let mut args = Self {
            port: DEFAULT_PORT,
            data: PathBuf::from("relay-data"),
            name: "notein relay".to_owned(),
            advertise: None,
        };
        while let Some(flag) = raw.next() {
            match flag.as_str() {
                "--port" => {
                    let text = raw.next().ok_or(ArgError::Missing("--port"))?;
                    args.port = text.parse().map_err(|_| ArgError::BadPort(text))?;
                }
                "--data" => {
                    args.data = PathBuf::from(raw.next().ok_or(ArgError::Missing("--data"))?)
                }
                "--name" => args.name = raw.next().ok_or(ArgError::Missing("--name"))?,
                "--advertise" => {
                    args.advertise = Some(raw.next().ok_or(ArgError::Missing("--advertise"))?);
                }
                other => return Err(ArgError::Unknown(other.to_owned())),
            }
        }
        Ok(args)
    }
}

fn local_ipv4() -> Option<String> {
    // Connect a UDP socket outward; the chosen source address is the
    // one other hosts can reach.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:1").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let profile = DeviceProfile::load_or_create(&args.data.join("relay.key"), &args.name)?;
    let store = RelayStore::on_disk(args.data.join("shares"))?;
    let shares = store.len();
    let server = RelayServer::start(
        RelayConfig {
            key: profile.key().clone(),
            name: profile.name().to_owned(),
            port: args.port,
        },
        store,
    )?;
    let host = args
        .advertise
        .or_else(local_ipv4)
        .unwrap_or_else(|| "127.0.0.1".to_owned());
    let addr = RelayAddr {
        host: format!("{host}:{}", server.local_addr().port()),
        public: *profile.key().public(),
    };
    println!("notein relay listening on {}", server.local_addr());
    println!("{shares} share(s) on disk under {}", args.data.display());
    println!("invite address: {addr}");
    loop {
        std::thread::park();
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("{err}");
            eprintln!(
                "usage: notein-relay [--port N] [--data DIR] [--name NAME] [--advertise HOST]"
            );
            return ExitCode::FAILURE;
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("relay failed: {err}");
            ExitCode::FAILURE
        }
    }
}
