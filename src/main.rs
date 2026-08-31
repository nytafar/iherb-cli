use anyhow::Result;
use clap::Parser;
use iherb_cli::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    iherb_cli::run(Cli::parse()).await
}
