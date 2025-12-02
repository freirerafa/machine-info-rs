use anyhow::Result;
use sysinfo::{System, Disks};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use log::{debug, info};
use crate::model::{SystemInfo, Processor, Disk as DiskModel, GraphicCard, GraphicsUsage, GraphicsProcessUtilization, SystemStatus, Process, Camera, NvidiaInfo};
use crate::monitor::Monitor;
use std::path::Path;

#[cfg(feature = "v4l")]
use crate::camera::list_cameras;

#[cfg(not(feature = "v4l"))]
fn list_cameras() -> Vec<Camera> {
    vec![]
}

/// Represents a machine. Currently you can monitor global CPU/Memory usage, processes CPU usage and the
/// Nvidia GPU usage. You can also retrieve information about CPU, disks...
pub struct Machine {
    monitor: Monitor,
    nvml: Option<nvml_wrapper::Nvml>,
}


impl Machine {
    /// Creates a new instance of Machine. If not graphic card it will warn about it but not an error
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// let m = Machine::new();
    /// ```
    pub fn new() -> Machine{
        let nvml = match Nvml::init() {
            Ok(nvml) => {
                info!("Nvidia driver loaded");
                Some(nvml)
            },
            Err(error) => {
                debug!("Nvidia not available because {}", error);
                None
            }
        };
        Machine{
            monitor: Monitor::new(),
            nvml: nvml
        }
    }
    
    /// Retrieves full information about the computer
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// let m = Machine::new();
    /// println!("{:?}", m.system_info())
    /// ```
    pub fn system_info(& mut self) -> SystemInfo {
        let mut sys = System::new();
        sys.refresh_all();
        
        // Get CPU info - in sysinfo 0.37, we use cpus() to get all CPUs
        let cpus = sys.cpus();
        let processor = if let Some(cpu) = cpus.first() {
            Processor{
                frequency: cpu.frequency(),
                vendor: cpu.vendor_id().to_string(),
                brand: cpu.brand().to_string()
            }
        } else {
            Processor{
                frequency: 0,
                vendor: "Unknown".to_string(),
                brand: "Unknown".to_string()
            }
        };

        // Get disks using Disks struct
        let disks_list = Disks::new_with_refreshed_list();
        let mut disks = Vec::new();
        for disk in disks_list.list() {
            // Handle potential errors when converting disk names and file systems
            let disk_name = disk.name().to_str().unwrap_or("Unknown").to_string();
            let fs = disk.file_system().to_string_lossy().to_string();
            let mount_point = disk.mount_point().to_str().unwrap_or("Unknown").to_string();
            
            disks.push(DiskModel{
                name: disk_name,
                fs,
                storage_type: match disk.kind() {
                    sysinfo::DiskKind::HDD => "HDD".to_string(),
                    sysinfo::DiskKind::SSD => "SSD".to_string(),
                    _ => "Unknown".to_string()
                },
                available: disk.available_space(),
                size: disk.total_space(),
                mount_point
            })
        }

        let mut cards = Vec::new();
        let nvidia = if let Some(nvml) = &self.nvml {
            // Handle device_count() error
            let device_count = match nvml.device_count() {
                Ok(count) => count,
                Err(e) => {
                    debug!("Failed to get NVIDIA device count: {}", e);
                    0
                }
            };
            
            for n in 0..device_count {
                // Handle device_by_index() error
                let device = match nvml.device_by_index(n) {
                    Ok(dev) => dev,
                    Err(e) => {
                        debug!("Failed to get NVIDIA device at index {}: {}", n, e);
                        continue;
                    }
                };
                
                // Handle brand() error gracefully - it may return UnexpectedVariant for new GPU brands
                // The error can occur when NVML returns a brand value that isn't in the enum yet
                let brand_str = match device.brand() {
                    Ok(brand) => match brand {
                        nvml_wrapper::enum_wrappers::device::Brand::GeForce => "GeForce".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::Quadro => "Quadro".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::Tesla => "Tesla".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::Titan => "Titan".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::NVS => "NVS".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::GRID => "GRID".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::VApps => "VApps".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::VPC => "VPC".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::VCS => "VCS".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::VWS => "VWS".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::CloudGaming => "CloudGaming".to_string(),
                        nvml_wrapper::enum_wrappers::device::Brand::Unknown => "Unknown".to_string(),
                        // Handle any future brand variants
                        _ => format!("{:?}", brand),
                    },
                    Err(e) => {
                        // This handles cases where NVML returns an unknown brand variant (e.g., variant 12)
                        // which can happen with newer GPU models not yet in the enum
                        debug!("Failed to get GPU brand (likely UnexpectedVariant): {}", e);
                        format!("Unknown(Error: {})", e)
                    }
                };
                
                // Handle other device operations with error handling
                let uuid = match device.uuid() {
                    Ok(u) => u,
                    Err(e) => {
                        debug!("Failed to get GPU UUID: {}", e);
                        continue;
                    }
                };
                
                let name = match device.name() {
                    Ok(n) => n,
                    Err(e) => {
                        debug!("Failed to get GPU name: {}", e);
                        continue;
                    }
                };
                
                let memory = match device.memory_info() {
                    Ok(m) => m.total,
                    Err(e) => {
                        debug!("Failed to get GPU memory info: {}", e);
                        continue;
                    }
                };
                
                let temperature = match device.temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu) {
                    Ok(t) => t,
                    Err(e) => {
                        debug!("Failed to get GPU temperature: {}", e);
                        continue;
                    }
                };
                
                cards.push(GraphicCard{
                    id: uuid,
                    name,
                    brand: brand_str,
                    memory,
                    temperature
                });
            }
            
            // Handle NvidiaInfo creation with error handling
            let nvidia_info = match (
                nvml.sys_driver_version(),
                nvml.sys_nvml_version(),
                nvml.sys_cuda_driver_version()
            ) {
                (Ok(driver), Ok(nvml_ver), Ok(cuda)) => Some(NvidiaInfo {
                    driver_version: driver,
                    nvml_version: nvml_ver,
                    cuda_version: cuda
                }),
                _ => {
                    debug!("Failed to get some NVIDIA system info");
                    None
                }
            };
            nvidia_info
        } else {
            None
        };
        
        // Getting the model
        let model_path = Path::new("/sys/firmware/devicetree/base/model");
        let model = if model_path.exists() {
            std::fs::read_to_string(model_path)
                .map_err(|e| {
                    debug!("Failed to read model path: {}", e);
                    e
                })
                .ok()
        } else {
            None
        };
        
        let vaapi = Path::new("/dev/dri/renderD128").exists();

        SystemInfo {
            os_name: System::name().unwrap_or_else(|| "Unknown".to_string()),
            kernel_version: System::kernel_version().unwrap_or_else(|| "Unknown".to_string()),
            os_version: System::os_version().unwrap_or_else(|| "Unknown".to_string()),
            distribution: System::distribution_id(),
            hostname: System::host_name().unwrap_or_else(|| "Unknown".to_string()),
            memory: sys.total_memory(),
            nvidia,
            vaapi,
            processor,
            total_processors: sys.cpus().len(),
            graphics: cards,
            disks,
            cameras: list_cameras(),
            model
        }
    }

    /*pub fn disks_status(&self) {
        //TODO
        /*
        let mut disks = Vec::new();
        for disk in self.sys.disks() {
            disks.push(api::model::Disk{
            })
            */
    }*/

    /// The current usage of all graphic cards (if any)
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// let m = Machine::new();
    /// println!("{:?}", m.graphics_status())
    /// ```
    pub fn graphics_status(&self) -> Vec<GraphicsUsage> {
        let mut cards = Vec::new();
        if let Some(nvml) = &self.nvml {
            // Handle device_count() error
            let device_count = match nvml.device_count() {
                Ok(count) => count,
                Err(e) => {
                    debug!("Failed to get NVIDIA device count in graphics_status: {}", e);
                    return cards;
                }
            };
            
            for n in 0..device_count {
                // Handle device_by_index() error
                let device = match nvml.device_by_index(n) {
                    Ok(dev) => dev,
                    Err(e) => {
                        debug!("Failed to get NVIDIA device at index {} in graphics_status: {}", n, e);
                        continue;
                    }
                };
                
                let mut processes = Vec::new();
                let stats = device.process_utilization_stats(None);
                if let Ok(stats) = stats {
                    for p in stats {
                        processes.push(GraphicsProcessUtilization{
                            pid: p.pid,
                            gpu: p.sm_util,
                            memory: p.mem_util,
                            encoder: p.enc_util,
                            decoder: p.dec_util
                        });
                    }
                }
    
                // Handle all device operations with error handling
                let uuid = match device.uuid() {
                    Ok(u) => u,
                    Err(e) => {
                        debug!("Failed to get GPU UUID in graphics_status: {}", e);
                        continue;
                    }
                };
                
                let memory_info = match device.memory_info() {
                    Ok(m) => m.used,
                    Err(e) => {
                        debug!("Failed to get GPU memory info in graphics_status: {}", e);
                        continue;
                    }
                };
                
                let encoder = match device.encoder_utilization() {
                    Ok(e) => e.utilization,
                    Err(e) => {
                        debug!("Failed to get GPU encoder utilization: {}", e);
                        continue;
                    }
                };
                
                let decoder = match device.decoder_utilization() {
                    Ok(d) => d.utilization,
                    Err(e) => {
                        debug!("Failed to get GPU decoder utilization: {}", e);
                        continue;
                    }
                };
                
                let utilization_rates = match device.utilization_rates() {
                    Ok(r) => r,
                    Err(e) => {
                        debug!("Failed to get GPU utilization rates: {}", e);
                        continue;
                    }
                };
                
                let temperature = match device.temperature(TemperatureSensor::Gpu) {
                    Ok(t) => t,
                    Err(e) => {
                        debug!("Failed to get GPU temperature in graphics_status: {}", e);
                        continue;
                    }
                };
                
                cards.push(GraphicsUsage {
                    id: uuid,
                    memory_used: memory_info,
                    encoder,
                    decoder,
                    gpu: utilization_rates.gpu,
                    memory_usage: utilization_rates.memory,
                    temperature,
                    processes
                });
            }
        }
        
        cards
        
    }


    /// To calculate the CPU usage of a process we have to keep track in time the process so first we have to register the process.
    /// You need to know the PID of your process and use it as parameters. In case you provide an invalid PID it will return error
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// let m = Machine::new();
    /// let process_pid = 3218;
    /// m.track_process(process_pid)
    /// ```
    pub fn track_process(&mut self, pid: i32) -> Result<()>{
        self.monitor.track_process(pid)
    }

    /// Once we dont need to track a process it is recommended to not keep using resources on it. You should know the PID of your process.
    /// If the PID was not registered before, it will just do nothing
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// let m = Machine::new();
    /// let process_pid = 3218;
    /// m.track_process(process_pid)
    /// m.untrack_process(process_pid)
    /// ```
    pub fn untrack_process(&mut self, pid: i32) {
        self.monitor.untrack_process(pid);
    }

    /// The CPU usage of all tracked processes since the last call. So if you call it every 10 seconds, you will
    /// get the CPU usage during the last 10 seconds. More calls will make the value more accurate but also more expensive
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// use std::{thread, time};
    /// 
    /// let m = Machine::new();
    /// m.track_process(3218)
    /// m.track_process(4467)
    /// loop {   
    ///   let status = m.processes_status();
    ///   println!("{:?}", status);
    ///   thread::sleep(time::Duration::from_millis(1000));
    /// }
    /// 
    /// ```
    pub fn processes_status(& mut self) -> Vec<Process> {
        self.monitor.next_processes().iter().map(|(pid, cpu)| Process{pid:*pid, cpu:*cpu}).collect::<Vec<Process>>()
    }

    /// The CPU and memory usage. For the CPU, it is the same as for `processes_status`. For the memory it returs the amount
    /// a this moment
    /// Example
    /// ```
    /// use machine_info::Machine;
    /// use std::{thread, time};
    /// 
    /// let m = Machine::new();
    /// m.track_process(3218)
    /// m.track_process(4467)
    /// loop {   
    ///   let status = m.system_status();
    ///   println!("{:?}", status);
    ///   thread::sleep(time::Duration::from_millis(1000));
    /// }
    /// 
    /// ```
    pub fn system_status(& mut self) -> Result<SystemStatus> {
        let (cpu, memory) = self.monitor.next()?;
        Ok(SystemStatus {
            memory,
            cpu,
        })
    }

}