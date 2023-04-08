use std::net::{SocketAddrV4, Ipv4Addr};
use std::env::{self, VarError};
use simplelog::{SimpleLogger, Config as LogConfig};
use tiny_http::{Response, Server};
use prometheus::{TextEncoder, Encoder};
use log::{info, LevelFilter};

mod docker;
mod collector;

pub struct Config {
    port: u16,
    min_log_level: LevelFilter,
    pub collect_image_metrics: bool,
    pub collect_volume_metrics: bool
}

impl Config {
    fn is_truthy(var: Result<String, VarError>, default: bool) -> bool {
        match var {
            Ok(s) => s == "1" || s.eq_ignore_ascii_case("true"),
            _ => default
        }
    }

    fn new() -> Config {
        Config {
            port: 9417,
            min_log_level: if Self::is_truthy(env::var("VERBOSE"), cfg!(debug_assertions)) { LevelFilter::Debug } else { LevelFilter::Info },
            collect_image_metrics: Self::is_truthy(env::var("COLLECT_IMAGE_METRICS"), cfg!(debug_assertions)),
            collect_volume_metrics: Self::is_truthy(env::var("COLLECT_VOLUME_METRICS"), cfg!(debug_assertions))
        }
    }
}

#[tokio::main]
async fn main() {
    ctrlc::set_handler(|| {
        info!("Exiting.");
        std::process::exit(0);
    }).unwrap();
    
    let config = Config::new();
    SimpleLogger::init(config.min_log_level, LogConfig::default()).unwrap();

    docker::get_data_usage().await.expect("Test Docker socket query failed.");

    let mut collector = collector::Collector::new();
    
    let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), config.port);
    let server = Server::http(addr).unwrap();

    for req in server.incoming_requests() {
        if req.url() != "/metrics" {
            req.respond(Response::empty(404)).unwrap_or(());
            continue;
        }

        if collector.update(&config).await {
            let mut buffer = Vec::new();
            let encoder = TextEncoder::new();
            encoder.encode(&prometheus::gather(), &mut buffer).unwrap();

            req.respond(Response::from_data(buffer)).unwrap_or(());
        } else {
            req.respond(Response::empty(408)).unwrap_or(());
        }
    }
}