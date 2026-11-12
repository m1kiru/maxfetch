use clap::Parser;

use maxfetch::cli::{Cli, run};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    run(cli).await
}
