use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "aurifer",
    version,
    about = "XAU/USD strategy harness (IG Markets)",
    long_about = "XAU/USD strategy harness (IG Markets).\n\n\
                  Save API credentials and settings persistently with:\n  \
                  aurifer config set <KEY> <VALUE>\n\n\
                  Run `aurifer config list` to see what's saved.",
    after_help = "EXAMPLES:\n  \
        aurifer gui                          # launch the GUI\n  \
        aurifer backtest --start 2020-01-01  # run a backtest\n  \
        aurifer config set IG_API_KEY <key>  # save a credential\n  \
        aurifer config list                  # show saved credentials\n  \
        aurifer config path                  # where they are stored"
)]
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
    /// Manage saved credentials and settings.
    Config {
        #[command(subcommand)]
        sub: ConfigCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum DiagnosticsCommand {
    PointPrediction,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Save a value persistently (e.g. `IG_API_KEY`).
    Set {
        /// Environment-variable name (e.g. IG_API_KEY).
        key: String,
        /// Value. If omitted (or `-`), read from stdin — useful for secrets
        /// that shouldn't appear in shell history.
        value: Option<String>,
    },
    /// Print a stored value.
    Get { key: String },
    /// List all stored keys (secrets are masked by default).
    List {
        #[arg(long)]
        show_secrets: bool,
    },
    /// Remove a stored value.
    Unset { key: String },
    /// Print the path of the config file.
    Path,
}