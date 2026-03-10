use std::collections::HashSet;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

pub mod rqt2_api {
    tonic::include_proto!("rqt2.api.v1");
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("rqt2_descriptor");
}

use rqt2_api::package_service_server::PackageService;
use rqt2_api::{InstallRequest, InstallProgress, PackageInfo, ListPackagesRequest};

pub async fn get_ros_distro() -> String {
    let output = Command::new("/bin/bash")
        .arg("-c")
        .arg("source /opt/ros/jazzy/setup.bash && echo $ROS_DISTRO")
        .output()
        .await;

    match output {
        Ok(out) => {
            let distro = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if distro.is_empty() { "Ninguna".into() } else { distro }
        },
        Err(_) => "No detectada".into(),
    }
}

async fn get_all_installed_matching_prefixes() -> HashSet<String> {
    let mut installed = HashSet::new();
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Package} ${Status}\n", "ros-*", "rti-*", "python3-*"])
        .output()
        .await;

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if line.contains("install ok installed") {
                if let Some(name) = line.split_whitespace().next() {
                    installed.insert(name.to_string());
                }
            }
        }
    }
    installed
}

async fn check_if_installed(pkg: &str) -> bool {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", pkg])
        .output()
        .await;
    
    output.map(|o| String::from_utf8_lossy(&o.stdout).contains("install ok installed"))
          .unwrap_or(false)
}

pub struct MyPackageService {
    is_installing: Arc<Mutex<bool>>,
}

impl Default for MyPackageService {
    fn default() -> Self {
        Self {
            is_installing: Arc::new(Mutex::new(false)),
        }
    }
}

#[tonic::async_trait]
impl PackageService for MyPackageService {
    type ListAvailablePackagesStream = ReceiverStream<Result<PackageInfo, Status>>;
    type InstallPackageStream = ReceiverStream<Result<InstallProgress, Status>>;

    async fn list_available_packages(
        &self,
        req: Request<ListPackagesRequest>,
    ) -> Result<Response<Self::ListAvailablePackagesStream>, Status> {
        let user_filter = req.into_inner().filter;
        let regex_filter = if user_filter.is_empty() {
            r"^(ros|rti|python3)-".to_string()
        } else {
            format!(r"^(ros|rti|python3)-.*{}", user_filter)
        };

        let (tx, rx) = mpsc::channel(128);

        tokio::spawn(async move {
            let installed_set = get_all_installed_matching_prefixes().await;
            let current_distro = get_ros_distro().await;

            let output = Command::new("apt-cache")
                .args(["search", "--names-only", &regex_filter])
                .output()
                .await;

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                for line in stdout.lines() {
                    if let Some((name, desc)) = line.split_once(" - ") {
                        let clean_name = name.trim();
                        let pkg = PackageInfo {
                            name: clean_name.to_string(),
                            description: desc.trim().to_string(),
                            version: current_distro.clone(),
                            is_installed: installed_set.contains(clean_name),
                        };
                    
                        if tx.send(Ok(pkg)).await.is_err() { break; }
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn install_package(
        &self,
        req: Request<InstallRequest>,
    ) -> Result<Response<Self::InstallPackageStream>, Status> {
        let mut lock = self.is_installing.lock().await;
        if *lock {
            return Err(Status::aborted("APT ocupado"));
        }

        *lock = true;
        let is_installing_flag = Arc::clone(&self.is_installing);
        let pkg_name = req.into_inner().package_name;
        let is_installed = check_if_installed(&pkg_name).await;
        let action = if is_installed { "remove" } else { "install" };
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let mut child = Command::new("pkexec")
                .args(["apt-get", action, "-y", &pkg_name])
                .spawn()
                .expect("Error al lanzar pkexec");

            let status = child.wait().await;
            let mut lock = is_installing_flag.lock().await;
            *lock = false;

            let final_msg = match status {
                Ok(s) if s.success() => "SUCCESS_COMPLETE",
                _ => "ERROR_CANCELLED",
            };

            let _ = tx.send(Ok(InstallProgress {
                log_line: final_msg.to_string(),
                progress: 100.0,
            })).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
