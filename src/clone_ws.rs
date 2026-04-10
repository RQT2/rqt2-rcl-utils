use std::path::{Path, PathBuf};
use std::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::package_service::rqt2_api;
use rqt2_api::clone_workspace_service_server::CloneWorkspaceService;
use rqt2_api::{CloneWorkspaceProgress, CloneWorkspaceRequest};

pub struct MyCloneWorkspaceService {
    last_target_dir: Arc<Mutex<Option<PathBuf>>>,
}

impl MyCloneWorkspaceService {
    fn workspace_state_file() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("rqt2")
            .join("target_dir.txt")
    }
}

impl Default for MyCloneWorkspaceService {
    fn default() -> Self {
        Self {
            last_target_dir: Arc::new(Mutex::new(None)),
        }
    }
}

fn expand_home_dir(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }

    if input == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }

    input.to_string()
}

fn extract_progress(line: &str) -> f32 {
    // git clone emite progreso como "Receiving objects:  42% (...)"
    for token in line.split_whitespace() {
        if let Some(raw) = token.strip_suffix('%') {
            if let Ok(value) = raw.parse::<f32>() {
                return value.clamp(0.0, 100.0);
            }
        }
    }
    0.0
}

#[tonic::async_trait]
impl CloneWorkspaceService for MyCloneWorkspaceService {
    type CloneWorkspaceStream = ReceiverStream<Result<CloneWorkspaceProgress, Status>>;

    async fn clone_workspace(
        &self,
        req: Request<CloneWorkspaceRequest>,
    ) -> Result<Response<Self::CloneWorkspaceStream>, Status> {
        let payload = req.into_inner();
        let repository_url = payload.repository_url.trim().to_string();
        let destination_dir = payload.destination_dir.trim().to_string();
        let workspace_name = payload.workspace_name.trim().to_string();

        if repository_url.is_empty() {
            return Err(Status::invalid_argument("repository_url es obligatorio"));
        }

        if workspace_name.contains('/') || workspace_name.contains("..") {
            return Err(Status::invalid_argument("workspace_name contiene caracteres no permitidos"));
        }

        let branch = payload.branch.trim().to_string();
        let depth = payload.depth;
        let last_target_dir = Arc::clone(&self.last_target_dir);
        let (tx, rx) = mpsc::channel(128);

        tokio::spawn(async move {
            let expanded_dir = if destination_dir.is_empty() {
                std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
            } else {
                expand_home_dir(&destination_dir)
            };
            let base_dir = PathBuf::from(expanded_dir);

            if let Err(e) = fs::create_dir_all(&base_dir) {
                let _ = tx.send(Ok(CloneWorkspaceProgress {
                    log_line: format!("No se pudo preparar el destino {}: {e}", base_dir.display()),
                    progress: 0.0,
                    completed: true,
                    success: false,
                })).await;
                return;
            }

            if !Path::new(&base_dir).is_dir() {
                let _ = tx.send(Ok(CloneWorkspaceProgress {
                    log_line: format!("Destino inválido: {}", base_dir.display()),
                    progress: 0.0,
                    completed: true,
                    success: false,
                })).await;
                return;
            }

            let _ = tx.send(Ok(CloneWorkspaceProgress {
                log_line: "Validando URL del repositorio...".to_string(),
                progress: 10.0,
                completed: false,
                success: false,
            })).await;

            let mut verify_cmd = Command::new("git");
            verify_cmd.args(&["ls-remote", "--heads", &repository_url]);
            verify_cmd.stdout(std::process::Stdio::piped());
            verify_cmd.stderr(std::process::Stdio::piped());

            match verify_cmd.output().await {
                Ok(output) if output.status.success() => {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: "URL válida. Iniciando clonado...".to_string(),
                        progress: 15.0,
                        completed: false,
                        success: false,
                    })).await;
                }
                Ok(_) => {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: "URL inválida o inaccesible. Verifica la dirección del repositorio.".to_string(),
                        progress: 0.0,
                        completed: true,
                        success: false,
                    })).await;
                    return;
                }
                Err(e) => {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: format!("Error validando URL: {e}"),
                        progress: 0.0,
                        completed: true,
                        success: false,
                    })).await;
                    return;
                }
            }

            let mut args = vec!["clone".to_string(), "--progress".to_string()];
            if !branch.is_empty() {
                args.push("--branch".to_string());
                args.push(branch);
            }
            if depth > 0 {
                args.push("--depth".to_string());
                args.push(depth.to_string());
            }
            args.push(repository_url);

            let mut cmd = Command::new("git");
            cmd.args(&args);
            cmd.current_dir(&base_dir);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            if !workspace_name.is_empty() {
                let target_dir = base_dir.join(&workspace_name);
                if target_dir.exists() {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: format!("Ya existe la carpeta: {}", target_dir.display()),
                        progress: 0.0,
                        completed: true,
                        success: false,
                    })).await;
                    return;
                }

                cmd.arg(target_dir.to_string_lossy().to_string());
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: format!("No se pudo ejecutar git: {e}"),
                        progress: 0.0,
                        completed: true,
                        success: false,
                    })).await;
                    return;
                }
            };

            if let Some(stderr) = child.stderr.take() {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let progress = extract_progress(&line);
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: line,
                        progress,
                        completed: false,
                        success: false,
                    })).await;
                }
            }

            let result = child.wait().await;
            match result {
                Ok(status) if status.success() => {
                    let final_target_dir = if workspace_name.is_empty() {
                        base_dir.clone()
                    } else {
                        base_dir.join(&workspace_name)
                    };

                    let state_file = MyCloneWorkspaceService::workspace_state_file();
                    if let Some(parent) = state_file.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&state_file, final_target_dir.to_string_lossy().as_bytes());
                    let mut lock = last_target_dir.lock().await;
                    *lock = Some(final_target_dir.clone());

                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: format!("{final_target_dir:?}"),
                        progress: 100.0,
                        completed: true,
                        success: true,
                    })).await;
                }
                Ok(status) => {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: format!("git clone terminó con error: {status}"),
                        progress: 100.0,
                        completed: true,
                        success: false,
                    })).await;
                }
                Err(e) => {
                    let _ = tx.send(Ok(CloneWorkspaceProgress {
                        log_line: format!("Fallo esperando proceso git clone: {e}"),
                        progress: 100.0,
                        completed: true,
                        success: false,
                    })).await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
