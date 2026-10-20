use anyhow::{Result, ensure};
use std::io::BufRead;
use track_builder::simulation::{VERSION, storage::Store, worker};
fn main() -> Result<()> {
    let line = std::io::stdin()
        .lock()
        .lines()
        .next()
        .transpose()?
        .unwrap_or_default();
    let request: worker::Request = serde_json::from_str(&line)?;
    ensure!(request.version == VERSION, "Unsupported worker protocol");
    let store = match std::env::var_os("CATS_SIMULATION_ROOT") {
        Some(p) => Store::at(p.into()),
        None => Store::default_location()?,
    };
    let progress = worker::run(&store, &request.job)?;
    println!("{}", serde_json::to_string(&progress)?);
    Ok(())
}
