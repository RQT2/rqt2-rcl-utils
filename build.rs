use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let proto_files = &["external/rqt2-api/proto/packages.proto"];
    let proto_dirs = &["external/rqt2-api/proto"];

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(out_dir.join("rqt2_descriptor.bin"))
        .compile(proto_files, proto_dirs)?;

    Ok(())
}
