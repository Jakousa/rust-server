fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().unwrap());
    println!("cargo:rerun-if-changed=proto/cluster.proto");
    tonic_build::configure()
        .compile_protos(&["proto/cluster.proto"], &["proto"])?;
    Ok(())
}
