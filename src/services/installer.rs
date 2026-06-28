use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use rqt2_api::rqt2::api::v1::ros_installer_service_server::RosInstallerService;
use rqt2_api::rqt2::api::v1::{EnvInstallProgress, EnvInstallRequest, StepStatus};

use crate::utils::admin::run_apt_install_sudo;

#[derive(Debug, Default)]
pub struct MyROSInstallerService;

type ResponseStream = Pin<
    Box<dyn tokio_stream::Stream<Item = Result<EnvInstallProgress, Status>> + Send>,
>;

#[tonic::async_trait]
impl RosInstallerService for MyROSInstallerService {
    type InstallEnvironmentStream = ResponseStream;

    async fn install_environment(
        &self,
        _request: Request<EnvInstallRequest>,
    ) -> Result<Response<Self::InstallEnvironmentStream>, Status> {
        let (tx, rx) = mpsc::channel(128);

        let home_dir = std::path::PathBuf::from(
            std::env::var("HOME").map_err(|_| {
                Status::internal("No se pudo determinar el directorio HOME del usuario")
            })?,
        );

        tokio::spawn(async move {
            if let Err(e) = run_installation_workflow(home_dir, tx).await {
                eprintln!("Error en el flujo de instalación: {:?}", e);
            }
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx)) as ResponseStream
        ))
    }
}

async fn run_installation_workflow(
    home: PathBuf,
    tx: mpsc::Sender<Result<EnvInstallProgress, Status>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_status(&tx, "CHECK_OS", "", StepStatus::Running, 5).await;
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    if !os_release.contains("Ubuntu")
        && !os_release.contains("Debian")
        && !os_release.contains("Mint")
    {
        send_status(
            &tx,
            "CHECK_OS_FAIL",
            "Distribución no soportada",
            StepStatus::Failed,
            5,
        )
        .await;
        return Ok(());
    }
    send_status(&tx, "CHECK_OS_SUCCESS", "", StepStatus::Running, 10).await;

    send_status(
        &tx,
        "INSTALL_DEPS",
        "Instalando dependencias base...",
        StepStatus::Running,
        15,
    )
    .await;

    let mut cmd = run_apt_install_sudo(&[
        "software-properties-common",
        "lsb-release",
        "gnupg",
        "curl",
    ])?;

    stream_command_output(&mut cmd, &tx, "INSTALL_DEPS", 20).await?;

    send_status(
        &tx,
        "DETECT_ROS_DISTRO",
        "Buscando versión compatible de ROS 2...",
        StepStatus::Running,
        40,
    )
    .await;
    let target_distros = vec!["jazzy", "humble", "rolling"];
    let mut selected_distro = "humble".to_string();

    for distro in target_distros {
        let output = Command::new("apt-cache")
            .args(&["search", &format!("ros-{}-desktop", distro)])
            .output()
            .await?;

        if !output.stdout.is_empty() {
            selected_distro = distro.to_string();
            break;
        }
    }
    send_status(
        &tx,
        "DETECT_ROS_DISTRO_SUCCESS",
        &format!("Seleccionada: {}", selected_distro),
        StepStatus::Running,
        45,
    )
    .await;

    let uros_ws = home.join("uros-ws");
    let uros_src = uros_ws.join("src");

    if uros_ws.exists() {
        let _ = tokio::fs::remove_dir_all(&uros_ws).await;
    }
    tokio::fs::create_dir_all(&uros_src).await?;

    send_status(
        &tx,
        "CLONE_MICROROS",
        "Clonando repositorio micro_ros_setup...",
        StepStatus::Running,
        60,
    )
    .await;

    let mut git_cmd = Command::new("git")
        .args(&[
            "clone",
            "-b",
            &selected_distro,
            "https://github.com/micro-ROS/micro_ros_setup.git",
            "micro_ros_setup",
        ])
        .current_dir(&uros_src)
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    stream_command_output(&mut git_cmd, &tx, "CLONE_MICROROS", 65).await?;

    send_status(
        &tx,
        "COLCON_BUILD",
        "Compilando entorno micro-ROS con colcon...",
        StepStatus::Running,
        80,
    )
    .await;

    let mut build_cmd = Command::new("bash")
        .args(&[
            "-c",
            &format!(
                "source /opt/ros/{}/setup.bash && colcon build",
                selected_distro
            ),
        ])
        .current_dir(&uros_ws)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    stream_command_output(&mut build_cmd, &tx, "COLCON_BUILD", 90).await?;

    send_status(
        &tx,
        "INSTALL_SUCCESS",
        "Entorno configurado correctamente.",
        StepStatus::Success,
        100,
    )
    .await;

    Ok(())
}

async fn stream_command_output(
    child: &mut tokio::process::Child,
    tx: &mpsc::Sender<Result<EnvInstallProgress, Status>>,
    step_id: &str,
    percentage: i32,
) -> Result<(), std::io::Error> {
    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout).lines();
        while let Some(line) = reader.next_line().await? {
            let _ = tx
                .send(Ok(EnvInstallProgress {
                    step_id: step_id.to_string(),
                    log_line: line,
                    status: StepStatus::Running as i32,
                    progress_percentage: percentage,
                }))
                .await;
        }
    }

    let _ = child.wait().await;
    Ok(())
}

async fn send_status(
    tx: &mpsc::Sender<Result<EnvInstallProgress, Status>>,
    step_id: &str,
    log: &str,
    status: StepStatus,
    percentage: i32,
) {
    let _ = tx
        .send(Ok(EnvInstallProgress {
            step_id: step_id.to_string(),
            log_line: log.to_string(),
            status: status as i32,
            progress_percentage: percentage,
        }))
        .await;
}
