use std::net::{SocketAddrV4, Ipv4Addr};
use std::env::{self, VarError};
use futures::future::join_all;
use tiny_http::{Response, Server};
use prometheus::{TextEncoder, Encoder, Gauge, Histogram, exponential_buckets, register_histogram, register_gauge, opts, labels};
use log::{info, debug};
use simplelog::*;

mod docker;

struct Options {
    port: u16,
    min_log_level: LevelFilter,
    // collect_image_metrics: bool,
    // collect_volume_metrics: bool
}

impl Options {
    fn is_truthy(var: Result<String, VarError>, default: bool) -> bool {
        match var {
            Ok(s) => s == "1" || s.eq_ignore_ascii_case("true"),
            _ => default
        }
    }

    fn new() -> Options {
        Options {
            port: 9417,
            min_log_level: if Self::is_truthy(env::var("VERBOSE"), cfg!(debug_assertions)) { LevelFilter::Debug } else { LevelFilter::Info },
            //collect_image_metrics: Self::is_truthy(env::var("COLLECT_IMAGE_METRICS"), false),
            //collect_volume_metrics: Self::is_truthy(env::var("COLLECT_VOLUME_METRICS"), false)
        }
    }
}

#[tokio::main]
async fn main() {
    ctrlc::set_handler(|| {
        info!("Exiting.");
        std::process::exit(0);
    }).unwrap();
    
    let options = Options::new();
    SimpleLogger::init(options.min_log_level, Config::default()).unwrap();

    docker::get_data_usage().await.expect("Test Docker socket query failed.");

    let mut logic = Logic::new();
    
    let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), options.port);
    let server = Server::http(addr).unwrap();

    for req in server.incoming_requests() {
        if req.url() != "/metrics" {
            req.respond(Response::empty(404)).unwrap_or(());
            continue;
        }

        if logic.update(&options).await {
            let mut buffer = Vec::new();
            let encoder = TextEncoder::new();
            encoder.encode(&prometheus::gather(), &mut buffer).unwrap();

            req.respond(Response::from_data(buffer)).unwrap_or(());
        } else {
            req.respond(Response::empty(408)).unwrap_or(());
        }
    }
}

struct ContainerTracker {
    id: String,
    cpu_usage: Gauge,
    memory_usage: Gauge,
    restart_count: Gauge,
    running_state: Gauge,
    start_time: Gauge,
    total_bytes_in: Gauge,
    total_bytes_out: Gauge,
    total_bytes_read: Gauge,
    total_bytes_written: Gauge
}

impl ContainerTracker {
    fn new(c: docker::Container) -> ContainerTracker {
        let name = Self::get_display_name(&c);
        let cpu_usage = register_gauge!(opts!("docker_container_cpu_used_total", "Accumulated CPU usage of a container, in unspecified units, averaged for all logical CPUs usable by the container.", labels! { "name" => &name })).unwrap();
        let memory_usage = register_gauge!(opts!("docker_container_memory_used_bytes", "Memory usage of a container.", labels! { "name" => &name })).unwrap();
        let restart_count = register_gauge!(opts!("docker_container_restart_count", "Number of times the runtime has restarted this container without explicit user action, since the container was last started.", labels! { "name" => &name })).unwrap();
        let running_state = register_gauge!(opts!("docker_container_running_state", "Whether the container is running (1), restarting (0.5) or stopped (0).", labels! { "name" => &name })).unwrap();
        let start_time = register_gauge!(opts!("docker_container_start_time_seconds", "Timestamp indicating when the container was started. Does not get reset by automatic restarts.", labels! { "name" => &name })).unwrap();
        let total_bytes_in = register_gauge!(opts!("docker_container_network_in_bytes", "Total bytes received by the container's network interfaces.", labels! { "name" => &name })).unwrap();
        let total_bytes_out = register_gauge!(opts!("docker_container_network_out_bytes", "Total bytes sent by the container's network interfaces.", labels! { "name" => &name })).unwrap();
        let total_bytes_read = register_gauge!(opts!("docker_container_disk_read_bytes", "Total bytes read from disk by a container.", labels! { "name" => &name })).unwrap();
        let total_bytes_written = register_gauge!(opts!("docker_container_disk_write_bytes", "Total bytes written to disk by a container.", labels! { "name" => &name })).unwrap();
        
        ContainerTracker {
            id: c.Id,
            cpu_usage,
            memory_usage,
            restart_count,
            running_state,
            start_time,
            total_bytes_in,
            total_bytes_out,
            total_bytes_read,
            total_bytes_written
        }
    }

    fn get_display_name(c: &docker::Container) -> String {
        match c.Names.first() {
            Some(name) if name.trim().len() > 1 => name.trim_start_matches('/').to_string(),
            _ => c.Id[..12].to_string()
        }
    }

    async fn update(&self, _options: &Options) -> Option<()> {
        let inspect = docker::inspect_container(&self.id).await?;

        self.running_state.set(if inspect.State.Running { 1. } else if inspect.State.Restarting { 0.5 } else { 0. });
        self.restart_count.set(inspect.RestartCount as f64);

        if let Ok(d) = chrono::DateTime::parse_from_rfc3339(&inspect.State.StartedAt) {
            let t = d.timestamp();

            if t > 0 {
                self.start_time.set(t as f64);
            }
        }

        if !inspect.State.Running {
            return Some(());
        }

        let stats = docker::get_container_stats(&self.id).await?;
        self.cpu_usage.set(stats.cpu_stats.cpu_usage.total_usage as f64);
        
        let tmp = stats.memory_stats.stats
            .get("total_inactive_file").copied()
            .or_else(|| stats.memory_stats.stats.get("inactive_file").copied())
            .unwrap_or_default();
        
        self.memory_usage.set((stats.memory_stats.usage - tmp) as f64);

        self.total_bytes_in.set(stats.networks.iter().map(|kvp| kvp.1.rx_bytes).sum::<u64>() as f64);
        self.total_bytes_out.set(stats.networks.iter().map(|kvp| kvp.1.tx_bytes).sum::<u64>() as f64);

        self.total_bytes_read.set(stats.blkio_stats.io_service_bytes_recursive.iter().filter_map(|s| if s.op.eq_ignore_ascii_case("read") { Some(s.value) } else { None }).sum::<u64>() as f64);
        self.total_bytes_written.set(stats.blkio_stats.io_service_bytes_recursive.iter().filter_map(|s| if s.op.eq_ignore_ascii_case("write") { Some(s.value) } else { None }).sum::<u64>() as f64);

        Some(())
    }
}

impl Drop for ContainerTracker {
    fn drop(&mut self) {
        debug!("Dropping container tracker {}", self.id);
        prometheus::unregister(Box::new(self.cpu_usage.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.memory_usage.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.restart_count.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.running_state.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.start_time.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.total_bytes_in.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.total_bytes_out.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.total_bytes_read.clone())).unwrap_or(());
        prometheus::unregister(Box::new(self.total_bytes_written.clone())).unwrap_or(());
    }
}

struct Logic {
    container_count: Gauge,
    probe_duration: Histogram,
    container_trackers: Vec<ContainerTracker>
}

impl Logic {
    fn new() -> Logic {
        let buckets = exponential_buckets(1.0, 2.0, 7).unwrap();
        let container_count = register_gauge!("docker_containers", "Number of containers that exist.").unwrap();
        let probe_duration = register_histogram!("docker_probe_duration_seconds", "How long it takes to query Docker for the complete data set. Includes failed requests.", buckets).unwrap();

        Logic {
            container_count,
            probe_duration,
            container_trackers: Vec::new()
        }
    }

    async fn update(&mut self, options: &Options) -> bool {
        debug!("Probing Docker.");

        let _timer = self.probe_duration.start_timer();

        match docker::list_containers().await {
            Some(listed_containers) => {
                self.container_trackers.retain(|c| { listed_containers.iter().any(|c2| { c.id == c2.Id }) });

                for c in listed_containers {
                    if !self.container_trackers.iter().any(|p| { p.id == c.Id }) {
                        debug!("Adding container tracker {}", c.Id);
                        self.container_trackers.push(ContainerTracker::new(c));
                    }
                }

                self.container_count.set(self.container_trackers.len() as f64);

                join_all(self.container_trackers.iter().map(|c| c.update(options))).await;
                true
            }
            _ => {
                false
            }
        }
    }
}