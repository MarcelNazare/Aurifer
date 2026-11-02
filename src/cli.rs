use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "aurifer", version, about = "XAU/USD strategy harness (IG Markets)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(long, default_value = "", global = true)]
    pub start: String,

    #[arg(long, default_value = "", global = true)]
    pub end: String,

    #[arg(long, default_value = "DAY", global = true)]
    pub resolution: String,

    #[arg(long, global = true)]
    pub no_news: bool,

    #[arg(long, global = true)]
    pub no_predictor: bool,

    #[arg(long, global = true)]
    pub no_open: bool,

    #[arg(long, global = true)]
    pub no_ig: bool,

    #[arg(long, global = true)]
    pub no_cache: bool,

    #[arg(long, global = true)]
    pub confirm_live: bool,

    #[arg(long, default_value = "5", global = true)]
    pub horizon: usize,

    #[arg(long, default_value = "0.25", global = true)]
    pub label_atr_mult: f64,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Backtest,
    Gui,
    Web,
    Live,
    Forecast,
    Diagnostics {
        #[command(subcommand)]
        sub: DiagnosticsCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum DiagnosticsCommand {
    PointPrediction,
}