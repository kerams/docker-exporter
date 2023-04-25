mod contract {
    #![allow(non_snake_case)]

    use std::collections::HashMap;
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Container {
        pub Id: String,
        pub Names: Vec<String>
    }

    #[derive(Deserialize)]
    pub struct ContainerState {
        pub Running: bool,
        pub Restarting: bool,
        #[serde(deserialize_with = "deserialize_null_default", default)]
        pub StartedAt: String
    }

    #[derive(Deserialize)]
    pub struct ContainerInspect {
        pub State: ContainerState,
        pub RestartCount: u32
    }

    #[derive(Deserialize)]
    pub struct MemoryStats {
        #[serde(deserialize_with = "deserialize_null_default", default)]
        pub stats: HashMap<String, u64>,
        #[serde(default)]
        pub usage: u64
    }

    #[derive(Default, Deserialize)]
    pub struct CpuUsage {
        pub total_usage: u64
    }

    #[derive(Deserialize)]
    pub struct CpuStats {
        pub cpu_usage: CpuUsage,
        #[serde(default)]
        pub system_cpu_usage: u64
    }

    #[derive(Deserialize)]
    pub struct Network {
        pub rx_bytes: u64,
        pub tx_bytes: u64
    }

    #[derive(Deserialize)]
    pub struct BlkioServiceBytesStat {
        pub op: String,
        pub value: u64
    }

    #[derive(Default, Deserialize)]
    pub struct BlkioStats {
        #[serde(deserialize_with = "deserialize_null_default", default)]
        pub io_service_bytes_recursive: Vec<BlkioServiceBytesStat>
    }

    #[derive(Deserialize)]
    pub struct ContainerStats {
        pub cpu_stats: CpuStats,
        pub memory_stats: MemoryStats,
        #[serde(deserialize_with = "deserialize_null_default", default)]
        pub networks: HashMap<String, Network>,
        #[serde(deserialize_with = "deserialize_null_default", default)]
        pub blkio_stats: BlkioStats
    }

    #[derive(Deserialize)]
    pub struct Image {
        pub Id: String,
        pub Containers: u32,
        pub RepoTags: Vec<String>,
        pub Size: u64
    }

    #[derive(Deserialize)]
    pub struct VolumeUsage {
        pub RefCount: u32,
        pub Size: u64
    }

    #[derive(Deserialize)]
    pub struct Volume {
        pub Name: String,
        pub UsageData: VolumeUsage
    }

    #[derive(Deserialize)]
    pub struct DataUsage {
        pub Images: Vec<Image>,
        pub Containers: Vec<Container>,
        pub Volumes: Vec<Volume>
    }

    fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        T: Default + Deserialize<'de>,
        D: serde::Deserializer<'de>,
    {
        let opt = Option::deserialize(deserializer)?;
        Ok(opt.unwrap_or_default())
    }
}

use std::future::Future;
use std::time::Duration;
use hyper::{body, Body, Client};
use hyperlocal::{UnixClientExt, Uri, UnixConnector};
use once_cell::sync::Lazy;
use log::error;
use tokio::select;
use tokio::time;

pub use contract::*;

static CLIENT: Lazy<Client<UnixConnector, Body>> = Lazy::new(|| { Client::unix() });

async fn get<T: serde::de::DeserializeOwned>(endpoint: &str) -> Option<T> {
    select! {
        () = time::sleep(Duration::from_secs(15)) => {
            error!("{} timed out.", endpoint);
            None
        }
        res = CLIENT.get(Uri::new("/var/run/docker.sock", endpoint).into()) => {
            match res {
                Ok(res) => {
                    let status = res.status();
                    
                    match body::to_bytes(res).await {
                        Ok(body) => {
                            if status.is_success() {
                                match serde_json::from_slice::<T>(&body) {
                                    Ok(res) => Some(res),
                                    Err(e) => {
                                        error!("{} deserialization error {} - {}", endpoint, e, String::from_utf8(body.to_vec()).unwrap());
                                        None
                                    }
                                }
                            } else {
                                error!("{} HTTP {} - {}", endpoint, status, String::from_utf8(body.to_vec()).unwrap());
                                None
                            }
                        }
                        Err(e) => {
                            error!("{} {}", endpoint, e);
                            None
                        }
                    }
                }
                Err(e) => {
                    error!("{} {}", endpoint, e);
                    None
                }
            }
        }
    }
}

pub fn list_containers() -> impl Future<Output = Option<Vec<Container>>> {
    get("/v1.25/containers/json?all=true")
}

pub async fn inspect_container(id: &str) -> Option<ContainerInspect> {
    get(format!("/v1.25/containers/{id}/json").as_str()).await
}

pub async fn get_container_stats(id: &str) -> Option<ContainerStats> {
    get(format!("/v1.25/containers/{id}/stats?stream=false").as_str()).await
}

pub fn get_data_usage() -> impl Future<Output = Option<DataUsage>> {
    get("/v1.25/system/df")
}