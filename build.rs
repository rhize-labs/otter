use std::io::Result;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=src/pb/badgerpb3.proto");
    println!("cargo:rerun-if-changed=src/pb/backup.proto");
    
    let mut config = prost_build::Config::new();
    // Don't set out_dir - let it use the default OUT_DIR
    
    println!("cargo:warning=Compiling protobuf files...");
    config.compile_protos(&["src/pb/badgerpb3.proto", "src/pb/backup.proto"], &["src/pb/"])?;
    println!("cargo:warning=Protobuf compilation completed!");
    
    Ok(())
}
