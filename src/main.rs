use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::io::IsTerminal;
use std::process::ExitCode;

mod decode;
mod diffs;
mod idl;
mod logs;
mod postmortem;
mod programs;
mod render;
mod rpc;
mod style;
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

    /// When to use colored output.
    /// `auto` (default): colors when stdout is a terminal and NO_COLOR is unset.
    /// `always` / `never` force the choice regardless.
    #[arg(long, value_enum, default_value = "auto")]
    color: ColorMode,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {e}", style::red_bold("solpm"));
            for cause in e.chain().skip(1) {
                eprintln!("  {} {cause}", style::dim("caused by:"));
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // Decide colors first so even error messages from below honour the choice.
    let use_color = match cli.color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            std::env::var_os("NO_COLOR").is_none()
                && !cli.json
                && std::io::stdout().is_terminal()
        }
    };
    style::set_enabled(use_color);

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
