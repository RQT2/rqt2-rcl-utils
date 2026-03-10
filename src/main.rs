use tonic::transport::Server;
use tonic_reflection::server::Builder;

mod package_service;

use package_service::{MyPackageService, get_ros_distro, rqt2_api};
use rqt2_api::package_service_server::PackageServiceServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:50051".parse()?;

    let reflection_service = Builder::configure()
        .register_encoded_file_descriptor_set(rqt2_api::FILE_DESCRIPTOR_SET)
        .build()?;
    
    let pkg_svc = PackageServiceServer::new(MyPackageService::default());

    println!(">_ RQT2-API Backend");
    println!("   {}@ROS2 {}", addr, get_ros_distro().await);

    Server::builder()
        .add_service(reflection_service)
        .add_service(pkg_svc)
        .serve(addr)
        .await?;

    Ok(())
}
