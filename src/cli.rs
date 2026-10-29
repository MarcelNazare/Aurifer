use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "aurifer", version, about = "XAU/USD strategy harness (IG Markets)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(long, default_value = "2024-01-01", global = true)] pub start: String,
    #[arg(long, default_value = "2024-03-01", global = true)] pub end: String,

    #[arg(long, global = true)] pub no_news: bool,
    #[arg(long, global = true)] pub no_predictor: bool,
    #[arg(long, global = true)] pub no_open: bool,
    #[arg(long, global = true)] pub no_ig: bool,
    #[arg(long, global = true)] pub confirm_live: bool,

    #[arg(long, default_value = "30", global = true)] pub horizon: usize,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Backtest, Tui, Web, Live,
    Diagnostics { #[command(subcommand)] sub: DiagnosticsCommand },
}

#[derive(Subcommand, Debug)]
pub enum DiagnosticsCommand {
    PointPrediction,
}