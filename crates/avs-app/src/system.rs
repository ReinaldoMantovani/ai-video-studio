//! Estado do hardware: CPU, memória, disco, GPU (quando detectável) e encoders.

use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use sysinfo::{Disks, System};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    #[ts(type = "number | null")]
    pub memory_total_mb: Option<u64>,
    #[ts(type = "number | null")]
    pub memory_used_mb: Option<u64>,
    pub utilization_pct: Option<f32>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SystemInfo {
    pub cpu_brand: String,
    pub cpu_cores: u32,
    pub cpu_usage_pct: f32,
    #[ts(type = "number")]
    pub memory_total: u64,
    #[ts(type = "number")]
    pub memory_used: u64,
    #[ts(type = "number")]
    pub disk_free: u64,
    pub gpus: Vec<GpuInfo>,
    pub ffmpeg_version: String,
    pub os: String,
}

pub struct SystemMonitor {
    sys: Mutex<System>,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        let mut s = System::new();
        s.refresh_cpu_all();
        s.refresh_memory();
        SystemMonitor { sys: Mutex::new(s) }
    }
}

impl SystemMonitor {
    pub fn snapshot(&self, library_root: &Path, ffmpeg_version: &str) -> SystemInfo {
        let mut s = self.sys.lock().unwrap();
        s.refresh_cpu_all();
        s.refresh_memory();
        let disks = Disks::new_with_refreshed_list();
        // Disco que contém a biblioteca: ponto de montagem mais longo que é prefixo.
        let disk_free = disks
            .list()
            .iter()
            .filter(|d| library_root.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(|d| d.available_space())
            .unwrap_or(0);
        SystemInfo {
            cpu_brand: s.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
            cpu_cores: s.cpus().len() as u32,
            cpu_usage_pct: s.global_cpu_usage(),
            memory_total: s.total_memory(),
            memory_used: s.used_memory(),
            disk_free,
            gpus: nvidia_gpus(),
            ffmpeg_version: ffmpeg_version.into(),
            os: format!("{} {}", System::name().unwrap_or_default(), System::os_version().unwrap_or_default()),
        }
    }
}

/// GPUs NVIDIA via `nvidia-smi` (quando instalado). Outros fabricantes
/// aparecem pelos encoders funcionais detectados em `hwaccel`.
fn nvidia_gpus() -> Vec<GpuInfo> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total,memory.used,utilization.gpu", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(|x| x.trim()).collect();
            (f.len() >= 4).then(|| GpuInfo {
                name: f[0].into(),
                vendor: "NVIDIA".into(),
                memory_total_mb: f[1].parse().ok(),
                memory_used_mb: f[2].parse().ok(),
                utilization_pct: f[3].parse().ok(),
            })
        })
        .collect()
}
