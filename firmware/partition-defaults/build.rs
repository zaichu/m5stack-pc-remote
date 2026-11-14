use std::env;
use std::fs;
use std::io;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let manifest_dir = Path::new(&manifest_dir);
    // manifest_dir is firmware/partition-defaults; parent is firmware/
    let firmware_dir = manifest_dir.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CARGO_MANIFEST_DIR has no parent directory",
        )
    })?;
    let partition_table = firmware_dir.join("partitions.csv");
    println!("cargo:rerun-if-changed={}", partition_table.display());

    let mut defaults =
        String::from("CONFIG_PARTITION_TABLE_CUSTOM=y\nCONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"");
    defaults.push_str(&partition_table.display().to_string());
    defaults.push_str("\"\n");

    let generated = firmware_dir.join("sdkconfig.partition-table.defaults");
    fs::write(&generated, defaults)?;
    println!("cargo:rerun-if-changed={}", generated.display());
    Ok(())
}
