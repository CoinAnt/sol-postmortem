use anyhow::Result;
use clap::Parser;

mod decode;
mod idl;
mod logs;
mod programs;
mod render;
mod rpc;

const DEFAULT_RPC: &str = "https://api.mainnet-beta.solana.com";

#[derive(Parser, Debug)]
#[command(
    name = "solpm",
    version,
    about = "Solana transaction postmortem — decode, trace, and explain any tx by signature."
)]
struct Cli {
    /// Transaction signature (base58)
    signature: String,

    /// RPC URL. Falls back to $SOLPM_RPC_URL, then mainnet-beta.
    #[arg(long)]
    rpc: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let rpc_url = cli
        .rpc
        .or_else(|| std::env::var("SOLPM_RPC_URL").ok())
        .unwrap_or_else(|| DEFAULT_RPC.to_string());

    let tx = rpc::fetch_transaction(&rpc_url, &cli.signature)?;
    render::print_postmortem(&rpc_url, &tx)?;

    Ok(())
}
