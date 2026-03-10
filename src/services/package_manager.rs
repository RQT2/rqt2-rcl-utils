use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tonic::{Request, Response, Status};
use crate::rqt2_api::{InstallRequest, InstallResponse, PackageInfo, ListPackagesRequest};
use tokio_stream::wrappers::ReceiverStream;
use tokio::sync::mpsc;

pub struct MyPackageService;

#[tonic::async_trait]
impl crate::rqt2_api::package_service_server::PackageService for MyPackageService {
    type ListAvailablePackagesStream = ReceiverStream<Result<PackageInfo, Status>>;
    type InstallPackageStream = ReceiverStream<Result<InstallProgress, Status>>;

    async fn list_available_packages(
        &self,
        req: Request<ListPackagesRequest>,
    ) -> Result<Response<Self::ListAvailablePackagesStream>, Status> {
        let filter = req.into_inner().filter;
        let (tx, rx) = mpsc::channel(128);

        tokio::spawn(async move {
            // Buscamos paquetes de ROS 2 Noble en el cache local
            let output = Command::new("apt-cache")
                .args(["search", &filter])
                .output()
                .await;

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                for line in stdout.lines() {
                    if let Some((name, desc)) = line.split_once(" - ") {
                        let pkg = PackageInfo {
                            name: name.trim().to_string(),
                            description: desc.trim().to_string(),
                            version: "latest".into(),
                            is_installed: check_if_installed(name.trim()).await,
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
        let pkg_name = req.into_inner().package_name;
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let mut child = Command::new("pkexec")
                .args(["apt-get", "install", "-y", &pkg_name])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| Status::internal(e.to_string()))?;

            let stdout = child.stdout.take().unwrap();
            let mut reader = BufReader::new(stdout).lines();

            // Enviamos cada línea de apt al frontend
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = tx.send(Ok(InstallProgress {
                    log_line: line,
                    progress: 0.0, // Aquí podrías parsear el avance
                })).await;
            }

            let _ = child.wait().await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn check_if_installed(pkg: &str) -> bool {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", pkg])
        .output()
        .await;
    
    output.map(|o| String::from_utf8_lossy(&o.stdout).contains("install ok installed"))
          .unwrap_or(false)
}
