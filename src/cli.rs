use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "aurifer", version, about = "XAU/USD strategy harness (IG Markets)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Start date (YYYY-MM-DD). Empty = 90 days before `end`.
    #[arg(long, default_value = "", global = true)] pub start: String,

    /// End date (YYYY-MM-DD). Empty = now (UTC).
    #[arg(long, default_value = "", global = true)] pub end: String,

    /// Bar resolution.
    /// HOUR    — ~2,160 points for 3 months (fits 10,000/week budget)
    /// MINUTE  — 10,000 points/week budget, covers ~6.9 days max
    #[arg(long, default_value = "HOUR", global = true)] pub resolution: String,

    #[arg(long, global = true)] pub no_news: bool,
    #[arg(long, global = true)] pub no_predictor: bool,
    #[arg(long, global = true)] pub no_open: bool,
    #[arg(long, global = true)] pub no_ig: bool,
    #[arg(long, global = true)] pub no_cache: bool,
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