use anyhow::{Context, Result};
use clap::Parser;

mod decode;
mod diffs;
mod idl;
mod logs;
mod postmortem;
mod programs;
mod render;
mod rpc;
mod tokens;

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

    /// Emit a single pretty-printed JSON object to stdout instead of the
    /// terminal-formatted view. Useful for piping into jq or other tools.
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let rpc_url = cli
        .rpc
        .or_else(|| std::env::var("SOLPM_RPC_URL").ok())
        .unwrap_or_else(|| DEFAULT_RPC.to_string());

    let tx = rpc::fetch_transaction(&rpc_url, &cli.signature)?;
    let pm = postmortem::assemble(&rpc_url, &cli.signature, &tx);

    if cli.json {
        let s = serde_json::to_string_pretty(&pm).context("serialise postmortem to JSON")?;
        println!("{s}");
    } else {
        render::print_pretty(&pm);
    }

    Ok(())
}
