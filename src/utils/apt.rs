use std::collections::HashSet;
use tokio::process::Command;

pub async fn get_ros_distro() -> String {
    let output = Command::new("/bin/bash")
        .arg("-c")
        .arg("source /opt/ros/jazzy/setup.bash && echo $ROS_DISTRO")
        .output()
        .await;

    match output {
        Ok(out) => {
            let distro = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if distro.is_empty() {
                "Ninguna".into()
            } else {
                distro
            }
        }
        Err(_) => "No detectada".into(),
    }
}

pub async fn get_all_installed_matching_prefixes() -> HashSet<String> {
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

pub async fn check_if_installed(pkg: &str) -> bool {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", pkg])
        .output()
        .await;

    output
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("install ok installed"))
        .unwrap_or(false)
}
