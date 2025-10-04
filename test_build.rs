use std::io::Result;

fn main() -> Result<()> {
    println!("Starting protobuf generation...");
    let mut config = prost_build::Config::new();
    config.out_dir("src/pb");
    println!("Config created, compiling protos...");
    config.compile_protos(&["src/pb/badgerpb3.proto", "src/pb/backup.proto"], &["src/pb/"])?;
    println!("Protobuf generation completed successfully!");
    Ok(())
}
