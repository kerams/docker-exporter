use log::debug;
use prometheus::{Counter, Gauge, register_gauge, Histogram, exponential_buckets, register_histogram, register_counter};
use crate::docker;
use crate::Config;

mod trackers {
    use log::debug;
    use prometheus::{opts, labels, Gauge, register_gauge};
    use crate::docker;

    pub struct ContainerTracker {
        pub id: String,
        cpu_usage: Gauge,
        cpu_capacity: Gauge,
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
        pub fn new(c: docker::Container) -> ContainerTracker {
            let name = Self::get_display_name(&c);
            let cpu_usage = register_gauge!(opts!("docker_container_cpu_used_total", "Accumulated CPU usage of a container, in unspecified units, averaged for all logical CPUs usable by the container.", labels! { "name" => &name })).unwrap();
            let cpu_capacity = register_gauge!(opts!("docker_container_cpu_capacity_total", "All potential CPU usage available to a container, in unspecified units, averaged for all logical CPUs usable by the container. Start point of measurement is undefined - only relative values should be used in analytics.", labels! { "name" => &name })).unwrap();
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
                cpu_capacity,
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

        pub async fn update(&self) -> Option<()> {
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
            self.cpu_capacity.set(stats.cpu_stats.system_cpu_usage as f64);
            
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

    pub struct VolumeTracker {
        pub name: String,
        size: Gauge,
        ref_count: Gauge
    }

    impl VolumeTracker {
        pub fn new(v: docker::Volume) -> VolumeTracker {
            let size = register_gauge!(opts!("docker_volume_size", "Size of a volume in bytes.", labels! { "name" => &v.Name })).unwrap();
            let ref_count = register_gauge!(opts!("docker_volume_container_count", "The number of containers using a volume.", labels! { "name" => &v.Name })).unwrap();
            
            let s = VolumeTracker {  
                name: v.Name.clone(),
                size,
                ref_count
            };

            Self::update(&s, &v);
            s
        }

        pub fn update(&self, v: &docker::Volume) {
            self.size.set(v.UsageData.Size as f64);
            self.ref_count.set(v.UsageData.RefCount as f64);
        }
    }

    impl Drop for VolumeTracker {
        fn drop(&mut self) {
            debug!("Dropping volume tracker {}", self.name);
            prometheus::unregister(Box::new(self.size.clone())).unwrap_or(());
            prometheus::unregister(Box::new(self.ref_count.clone())).unwrap_or(());
        }
    }

    pub struct ImageTracker {
        pub id: String,
        container_count: Gauge,
        size: Gauge
    }

    impl ImageTracker {
        pub fn new(i: docker::Image) -> ImageTracker {
            let tag = i.RepoTags.first().map(|x| x.to_string()).unwrap_or_else(|| i.Id.trim_start_matches("sha256:").to_string());
            let container_count = register_gauge!(opts!("docker_image_container_count", "The number of containers based on an image.", labels! { "tag" => &tag })).unwrap();
            let size = register_gauge!(opts!("docker_image_size", "The size of on an image in bytes.", labels! { "tag" => &tag })).unwrap();

            let s = ImageTracker {
                id: i.Id.clone(),
                container_count,
                size
            };

            Self::update(&s, &i);
            s
        }

        pub fn update(&self, i: &docker::Image) {
            self.container_count.set(i.Containers as f64);
            self.size.set(i.Size as f64);
        }
    }

    impl Drop for ImageTracker {
        fn drop(&mut self) {
            debug!("Dropping image tracker {}", self.id);
            prometheus::unregister(Box::new(self.container_count.clone())).unwrap_or(());
            prometheus::unregister(Box::new(self.size.clone())).unwrap_or(());
        }
    }
}

use trackers::*;

pub struct Collector {
    container_count: Gauge,
    probe_duration: Histogram,
    probe_failures: Counter,
    container_trackers: Vec<ContainerTracker>,
    volume_trackers: Vec<VolumeTracker>,
    image_trackers: Vec<ImageTracker>
}

impl Collector {
    pub fn new() -> Collector {
        let buckets = exponential_buckets(1.0, 2.0, 7).unwrap();
        let container_count = register_gauge!("docker_containers", "Number of containers that exist.").unwrap();
        let probe_duration = register_histogram!("docker_probe_duration_seconds", "How long it takes to query Docker for the complete data set.", buckets).unwrap();
        let probe_failures = register_counter!("docker_probe_failures_total", "The number of times any individual Docker query failed (because of a timeout or other reasons).").unwrap();

        Collector {
            container_count,
            probe_duration,
            probe_failures,
            container_trackers: Vec::new(),
            volume_trackers: Vec::new(),
            image_trackers: Vec::new()
        }
    }

    pub async fn update(&mut self, config: &Config) -> bool {
        debug!("Probing Docker.");

        let _timer = self.probe_duration.start_timer();

        let res = if config.collect_image_metrics || config.collect_volume_metrics {
            docker::get_data_usage().await.map(|x| (x.Containers, x.Volumes, x.Images))
        } else {
            // List only containers when we're not collecting images or volumes - it's faster
            docker::list_containers().await.map(|x| (x, Vec::new(), Vec::new()))
        };

        match res {
            Some((listed_containers, listed_volumes, listed_images)) => {
                self.container_trackers.retain(|c| listed_containers.iter().any(|c2| c.id == c2.Id));

                for c in listed_containers {
                    if !self.container_trackers.iter().any(|p| p.id == c.Id) {
                        debug!("Adding container tracker {}", c.Id);
                        self.container_trackers.push(ContainerTracker::new(c));
                    }
                }

                self.container_count.set(self.container_trackers.len() as f64);

                let update_results = futures::future::join_all(self.container_trackers.iter().map(|c| c.update())).await;

                match update_results.iter().filter(|x| x.is_none()).count() {
                    x if x > 0 => self.probe_failures.inc_by(x as f64),
                    _ => ()
                }

                if config.collect_volume_metrics {
                    self.volume_trackers.retain(|v| listed_volumes.iter().any(|v2| v.name == v2.Name));

                    for v in listed_volumes {
                        match self.volume_trackers.iter().find(|p| p.name == v.Name) {
                            Some(p) => p.update(&v),
                            _ => {
                                debug!("Adding volume tracker {}", v.Name);
                                self.volume_trackers.push(VolumeTracker::new(v));
                            }
                        }
                    }
                }

                if config.collect_image_metrics {
                    self.image_trackers.retain(|i| listed_images.iter().any(|i2| i.id == i2.Id));

                    for i in listed_images {
                        match self.image_trackers.iter().find(|p| p.id == i.Id) {
                            Some(p) => p.update(&i),
                            _ => {
                                debug!("Adding image tracker {}", i.Id);
                                self.image_trackers.push(ImageTracker::new(i));
                            }
                        }
                    }
                }

                true
            }
            _ => {
                self.probe_failures.inc();
                false
            }
        }
    }
}