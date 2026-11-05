#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::Parser;
use regex::Regex;

mod broker;
mod cli;
mod config_store;
mod costs;
mod data;
mod gui;
mod indicators;
mod point_predictor;
mod predictor;
mod session;
mod stats;
mod strategies;
mod types;
mod web;

use broker::IgBroker;
use cli::{Cli, Command, ConfigCommand, DiagnosticsCommand};
use predictor::{CalibratedPredictor, PredictorConfig};
use session::SessionState;
use types::*;

struct TrainingResult {
    model: CalibratedPredictor,
    validated: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    config_store::apply_to_env();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let args = Cli::parse();

    if let Some(Command::Config { sub }) = args.command.as_ref() {
        return handle_config_command(sub);
    }

    let (start, end) = resolve_date_range(&args.start, &args.end)?;

    let ig: Option<IgBroker> = if args.no_ig {
        None
    } else {
        match IgBroker::connect().await {
            Ok(mut b) => {
                b.set_no_cache(args.no_cache);
                println!("✅ IG connected — epic {}", b.epic());
                Some(b)
            }
            Err(e) => {
                eprintln!("⚠️ IG connection failed: {e}");
                eprintln!("   Persist IG credentials for future runs with:");
                eprintln!("     aurifer config set IG_API_KEY <your-key>");
                eprintln!("     aurifer config set IG_USERNAME <your-username>");
                eprintln!("     aurifer config set IG_PASSWORD <your-password>");
                eprintln!("   Or set them in `.env`. `aurifer config list` shows what's saved.");
                eprintln!("⚠️ Falling back to synthetic data. Set IG_* in .env to use IG.");
                None
            }
        }
    };

    match args.command.as_ref().unwrap_or(&Command::Gui) {
        Command::Forecast => {
            run_forecast(ig.as_ref(), &args, start, end).await?;
        }
        Command::Backtest => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let mut cfg = base_config(ig.is_none(), &args);
            let news_events = fetch_news_events_for_range(&args, start, end).await;
            cfg.news_blackouts = events_to_blackouts(&news_events);
            print_news_summary(&news_events, &cfg);
            let training = train_model(&bars, &cfg, &args)?;
            let trained = training.as_ref()
                .filter(|t| t.validated)
                .map(|t| t.model.clone());
            let mut s = make_session(cfg.clone(), trained);
            for b in &bars { s.step(b); }
            if let Some(l) = bars.last() { s.force_close(l); }
            print_report(&s);
        }
        Command::Gui => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let mut cfg = base_config(ig.is_none(), &args);
            let news_events = fetch_news_events_for_range(&args, start, end).await;
            cfg.news_blackouts = events_to_blackouts(&news_events);
            let warnings = build_warnings(&cfg, &args);
            let training = train_model(&bars, &cfg, &args)?;
            let trained = training.as_ref()
                .filter(|t| t.validated)
                .map(|t| t.model.clone());
            let forecast = training.as_ref().map(|t| {
                compute_forecast_summary(&bars, &cfg, &args, &t.model, t.validated)
            });
            gui::run_gui(bars, cfg, warnings, trained, forecast, news_events)?;
        }
        Command::Web => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let mut cfg = base_config(ig.is_none(), &args);
            let news_events = fetch_news_events_for_range(&args, start, end).await;
            cfg.news_blackouts = events_to_blackouts(&news_events);
            let warnings = build_warnings(&cfg, &args);
            let training = train_model(&bars, &cfg, &args)?;
            let trained = training.as_ref()
                .filter(|t| t.validated)
                .map(|t| t.model.clone());
            let forecast = training.as_ref().map(|t| {
                compute_forecast_summary(&bars, &cfg, &args, &t.model, t.validated)
            });
            web::run_web_server(
                bars, cfg, warnings, trained, forecast, news_events, !args.no_open,
            ).await?;
        }
        Command::Live => {
            if !args.confirm_live {
                anyhow::bail!("Live mode requires --confirm-live. This places REAL orders.");
            }
            let b = ig.context("live mode requires IG credentials in .env")?;
            let mut cfg = base_config(false, &args);
            let now = Utc::now();
            let news_events = fetch_news_events_for_range(
                &args, now - chrono::Duration::days(7), now,
            ).await;
            cfg.news_blackouts = events_to_blackouts(&news_events);
            let warm_end = Utc::now();
            let warm_start = warm_end - chrono::Duration::days(7);
            let bars = data::fetch_ig_bars(&b, "MINUTE", warm_start, warm_end).await?;
            let training = train_model(&bars, &cfg, &args)?;
            let trained = training.as_ref()
                .filter(|t| t.validated)
                .map(|t| t.model.clone());
            run_live_trading(b, cfg, trained).await?;
        }
        Command::Diagnostics { sub: DiagnosticsCommand::PointPrediction } => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let mut cfg = base_config(ig.is_none(), &args);
            let news_events = fetch_news_events_for_range(&args, start, end).await;
            cfg.news_blackouts = events_to_blackouts(&news_events);
            let mut pcfg = point_predictor::PointPredictorConfig::default();
            pcfg.horizon = args.horizon;
            run_point_prediction_diagnostic(&bars, &cfg, pcfg).await?;
        }
        Command::Config { .. } => unreachable!("handled above"),
    }
    Ok(())
}

// ── news fetching / conversion ─────────────────────────────────────

async fn fetch_news_events_for_range(
    args: &Cli,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Vec<NewsEvent> {
    if args.no_news {
        return Vec::new();
    }
    match fetch_news_events(start, end).await {
        Ok(events) => events,
        Err(e) => {
            eprintln!("  ⚠️  news fetch failed: {e}");
            Vec::new()
        }
    }
}

fn events_to_blackouts(events: &[NewsEvent]) -> Vec<(i64, i64)> {
    let mut w: Vec<(i64, i64)> = events.iter().map(|e| e.blackout()).collect();
    w.sort_by_key(|(s, _)| *s);
    w.dedup();
    w
}

fn print_news_summary(events: &[NewsEvent], cfg: &Config) {
    println!("\n── News ──");
    if events.is_empty() {
        println!("  no news events in range");
        return;
    }
    let fomc = events.iter()
        .filter(|e| matches!(e.kind, NewsKind::Fomc))
        .count();
    let news = events.len() - fomc;
    println!("  {} FOMC, {} headlines", fomc, news);
    println!("  {} blackout windows", cfg.news_blackouts.len());
    for e in events.iter().take(5) {
        let when = chrono::DateTime::from_timestamp(e.ts, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        println!("    {} [{}] {}", when, e.kind.as_str(), e.title);
    }
    if events.len() > 5 {
        println!("    … and {} more", events.len() - 5);
    }
}

// ── config subcommand ─────────────────────────────────────────────

fn handle_config_command(sub: &ConfigCommand) -> Result<()> {
    match sub {
        ConfigCommand::Set { key, value } => {
            let v = match value {
                Some(v) if !v.is_empty() && v != "-" => v.clone(),
                _ => read_value_from_stdin(key)?,
            };
            if v.trim().is_empty() {
                anyhow::bail!("empty value — nothing saved");
            }
            let path = config_store::set(key, &v)?;
            println!("✅ Saved {key} → {}", path.display());
            println!("   Future runs will use it automatically.");
        }
        ConfigCommand::Get { key } => match config_store::get(key)? {
            Some(v) => println!("{v}"),
            None => anyhow::bail!("{key} is not set"),
        },
        ConfigCommand::List { show_secrets } => {
            let cfg = config_store::load()?;
            if cfg.values.is_empty() {
                println!("(no saved configuration)");
                println!();
                println!("Known keys you may want to set:");
                for (k, desc) in config_store::KNOWN_KEYS {
                    println!("  {:<28} {}", k, desc);
                }
                println!();
                println!("Path: {}", config_store::config_path().display());
                return Ok(());
            }
            for (k, v) in &cfg.values {
                let shown = if !*show_secrets && config_store::is_secret(k) {
                    config_store::mask(v)
                } else {
                    v.clone()
                };
                println!("{k} = {shown}");
            }
            println!();
            println!("Path: {}", config_store::config_path().display());
        }
        ConfigCommand::Unset { key } => {
            let (removed, path) = config_store::unset(key)?;
            if removed {
                println!("✅ Removed {key} from {}", path.display());
            } else {
                println!("{key} was not set");
            }
        }
        ConfigCommand::Path => println!("{}", config_store::config_path().display()),
    }
    Ok(())
}

fn read_value_from_stdin(key: &str) -> Result<String> {
    use std::io::IsTerminal;
    use std::io::{BufRead, Read, Write};

    if std::io::stdin().is_terminal() {
        eprint!("Enter value for {key}: ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("reading value from stdin")?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading value from stdin")?;
        Ok(buf.trim().to_string())
    }
}

// ── training / report ─────────────────────────────────────────────

fn train_model(bars: &[Bar], cfg: &Config, args: &Cli) -> Result<Option<TrainingResult>> {
    if args.no_predictor {
        return Ok(None);
    }

    let mut warm = SessionState::new(cfg.clone());
    let mut evals = Vec::with_capacity(bars.len());
    for b in bars {
        warm.step(b);
        evals.push(warm.last_eval.clone().unwrap_or_else(default_eval));
    }
    let mut pcfg = PredictorConfig::default();
    pcfg.horizon = args.horizon;
    pcfg.label_atr_mult = args.label_atr_mult;

    println!("\n── Walk-forward validation ──");
    let wf = predictor::walk_forward_validate(bars, &evals, &pcfg, 4)?;
    let validated = wf.validate().is_ok();
    if validated {
        println!("  ✅ validation passed");
    } else {
        eprintln!("  ⚠️  validation failed — model used for forecast display only");
    }

    println!("\n── Full predictor training ──");
    let model = match CalibratedPredictor::train_calibrated(bars, &evals, pcfg) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("⚠️ predictor training failed: {e}");
            return Ok(None);
        }
    };

    Ok(Some(TrainingResult { model, validated }))
}

fn compute_forecast_summary(
    bars: &[Bar],
    cfg: &Config,
    args: &Cli,
    model: &CalibratedPredictor,
    validated: bool,
) -> ForecastSummary {
    let mut engine = strategies::StrategyEngine::with_predictor(cfg.clone(), model.clone());
    let mut last_eval = None;
    for b in bars {
        last_eval = Some(engine.on_bar(b));
    }
    let eval = match last_eval {
        Some(e) => e,
        None => {
            return ForecastSummary {
                horizon: args.horizon,
                resolution: args.resolution.clone(),
                last_date: "—".into(),
                last_close: 0.0,
                p_up: 0.5,
                direction: "NO DATA".into(),
                confidence: "LOW".into(),
                edge_pct: 0.0,
                auc: 0.5,
                auc_ci_lo: 0.5,
                auc_ci_hi: 0.5,
                brier: 0.25,
                base_rate: 0.5,
                validated: false,
                top_features: vec![],
            };
        }
    };
    let p_up = eval.prediction.unwrap_or(0.5);

    let last_bar = bars.last().unwrap();
    let last_date = chrono::DateTime::from_timestamp(last_bar.ts, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default();

    let direction = if p_up >= 0.60 {
        "LIKELY UP"
    } else if p_up <= 0.40 {
        "LIKELY DOWN"
    } else {
        "UNCERTAIN"
    };
    let confidence = if (p_up - 0.5).abs() > 0.15 {
        "HIGH"
    } else if (p_up - 0.5).abs() > 0.05 {
        "MEDIUM"
    } else {
        "LOW"
    };

    ForecastSummary {
        horizon: args.horizon,
        resolution: args.resolution.clone(),
        last_date,
        last_close: last_bar.close,
        p_up,
        direction: direction.into(),
        confidence: confidence.into(),
        edge_pct: (p_up - 0.5).abs() * 100.0,
        auc: model.metrics.auc,
        auc_ci_lo: model.auc_ci_95.0,
        auc_ci_hi: model.auc_ci_95.1,
        brier: model.brier_calibrated,
        base_rate: model.metrics.base_rate,
        validated,
        top_features: model
            .top_features_by_weight(3)
            .into_iter()
            .map(|(n, w)| (n.to_string(), w))
            .collect(),
    }
}

async fn run_forecast(
    ig: Option<&IgBroker>,
    args: &Cli,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    println!("═══════════════════════════════════════════════════════════");
    println!("  AURIFER FORECAST");
    println!("═══════════════════════════════════════════════════════════");
    println!(
        "  Window:     {} → {} ({} resolution)",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
        args.resolution
    );
    println!(
        "  Horizon:    {} bars, threshold {}× ATR",
        args.horizon, args.label_atr_mult
    );
    println!();

    let res = args.resolution.to_uppercase();
    let bars = if matches!(res.as_str(), "DAY" | "WEEK" | "MONTH") {
        data::fetch_csv_bars(&args.resolution, start, end).await?
    } else {
        let broker = ig.context("forecast at HOUR/MINUTE requires IG credentials")?;
        data::fetch_ig_bars(broker, &args.resolution, start, end).await?
    };
    println!("  Loaded:     {} bars", bars.len());

    if bars.len() < 1500 {
        anyhow::bail!(
            "only {} bars available — need at least 1,500 for a 2-fold walk-forward.\n\
             For DAY: aurifer forecast --start 2014-01-01",
            bars.len()
        );
    }
    if bars.len() < 2500 {
        eprintln!("⚠️  only {} bars (<2,500 ideal) — walk-forward will use fewer folds", bars.len());
        eprintln!();
    }

    let mut cfg = Config::default();
    let news_events = fetch_news_events_for_range(args, start, end).await;
    cfg.news_blackouts = events_to_blackouts(&news_events);
    let training = train_model(&bars, &cfg, args)?;
    let t = training.context("predictor disabled or training failed")?;
    let f = compute_forecast_summary(&bars, &cfg, args, &t.model, t.validated);

    println!();
    println!("───────────────────────────────────────────────────────────");
    println!("  As of:          {}  (last close)", f.last_date);
    println!("  Current price:  {:.2}", f.last_close);
    println!();
    println!(
        "  ─── Prediction: next {} {} bar{} ───",
        f.horizon,
        f.resolution,
        if f.horizon == 1 { "" } else { "s" }
    );
    println!("  P(up):          {:.1}%", f.p_up * 100.0);
    println!("  P(not up):      {:.1}%  (down or flat)", (1.0 - f.p_up) * 100.0);
    println!("  Direction:      {}", f.direction);
    println!("  Confidence:     {}  (edge: {:.1}% above 50%)", f.confidence, f.edge_pct);
    println!();
    println!("  ─── Model quality ───");
    if f.validated {
        println!("  Walk-forward:   ✅ VALIDATED");
    } else {
        println!("  Walk-forward:   ⚠️  UNVALIDATED");
        println!("  ⚠️  This forecast is for research only — do not trade it.");
    }
    println!("  AUC:            {:.3}  (95% CI {:.3}–{:.3})", f.auc, f.auc_ci_lo, f.auc_ci_hi);
    println!("  Brier:          {:.4}  ({})", f.brier,
             if f.brier < 0.24 { "good" } else { "poor" });
    println!("  Base rate:      {:.1}%", f.base_rate * 100.0);
    println!();
    println!("═══════════════════════════════════════════════════════════");
    Ok(())
}

async fn load_bars(
    ig: Option<&IgBroker>,
    start: &DateTime<Utc>,
    end: &DateTime<Utc>,
    args: &Cli,
) -> Result<Vec<Bar>> {
    println!(
        "📅 date range: {} → {} (resolution: {})",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
        args.resolution
    );

    let res = args.resolution.to_uppercase();

    if matches!(res.as_str(), "DAY" | "WEEK" | "MONTH") {
        let bars = data::fetch_csv_bars(&args.resolution, *start, *end).await?;
        println!("Loaded {} {} bars from CSV", bars.len(), args.resolution);
        if bars.is_empty() {
            anyhow::bail!("no bars returned from CSV");
        }
        return Ok(bars);
    }

    match ig {
        Some(b) => {
            let bars = data::fetch_ig_bars(b, &args.resolution, *start, *end).await?;
            println!("Loaded {} {} bars from IG", bars.len(), args.resolution);
            if bars.is_empty() {
                anyhow::bail!("IG returned no bars for this range");
            }
            Ok(bars)
        }
        None => {
            let bars = data::fetch_synthetic_bars(*start, *end).await?;
            println!("Loaded {} bars (synthetic)", bars.len());
            Ok(bars)
        }
    }
}

fn base_config(mid_price: bool, args: &Cli) -> Config {
    let mut cfg = Config::default();
    apply_resolution_defaults(&mut cfg, &args.resolution);
    cfg.spread = 0.30;
    cfg.commission_per_unit = 0.0;
    cfg.data_source_is_mid_price = mid_price;
    cfg
}

fn build_warnings(_cfg: &Config, args: &Cli) -> Vec<String> {
    let mut warnings = Vec::new();
    if _cfg.data_source_is_mid_price {
        warnings.push("synthetic data — all results meaningless".into());
    }
    if !matches!(
        args.resolution.to_uppercase().as_str(),
        "DAY" | "WEEK" | "MONTH"
    ) {
        warnings.push(format!(
            "resolution={} — live loop runs on MINUTE bars. \
             backtest validates the strategy idea, not the live execution.",
            args.resolution
        ));
    }
    warnings
}

fn resolve_date_range(start_str: &str, end_str: &str) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = if end_str.is_empty() {
        Utc::now()
    } else {
        parse_date(end_str)?
    };
    let start = if start_str.is_empty() {
        end - chrono::Duration::days(365 * 12)
    } else {
        parse_date(start_str)?
    };
    anyhow::ensure!(start < end, "start date must be before end date");
    anyhow::ensure!(
        end <= Utc::now() + chrono::Duration::days(1),
        "end date {} is in the future",
        end.format("%Y-%m-%d")
    );
    Ok((start, end))
}

fn apply_resolution_defaults(cfg: &mut Config, resolution: &str) {
    match resolution.to_uppercase().as_str() {
        "MINUTE" | "MINUTE_2" | "MINUTE_3" | "MINUTE_5" => {
            cfg.max_seconds_in_trade = 1800;
            cfg.max_bars_in_trade = 30;
        }
        "MINUTE_10" | "MINUTE_15" | "MINUTE_30" => {
            cfg.max_seconds_in_trade = 4 * 3600;
            cfg.max_bars_in_trade = 16;
        }
        "HOUR" | "HOUR_2" | "HOUR_3" | "HOUR_4" => {
            cfg.max_seconds_in_trade = 24 * 3600;
            cfg.max_bars_in_trade = 24;
        }
        "DAY" | "WEEK" => {
            cfg.max_seconds_in_trade = 7 * 24 * 3600;
            cfg.max_bars_in_trade = 7;
        }
        _ => {}
    }
}

fn make_session(cfg: Config, trained: Option<CalibratedPredictor>) -> SessionState {
    match trained {
        Some(t) => SessionState::with_predictor(cfg, t),
        None => SessionState::new(cfg),
    }
}

fn default_eval() -> strategies::Eval {
    strategies::Eval {
        bar_index: 0,
        atr: None,
        adx: None,
        rsi: None,
        trend_up: false,
        trend_ready: false,
        squeeze_ok: false,
        flow: None,
        signals: vec![],
        prediction: None,
    }
}

fn parse_date(s: &str) -> Result<DateTime<Utc>> {
    let n = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("bad date: {s} (expected YYYY-MM-DD)"))?;
    Ok(Utc.from_utc_datetime(&n.and_hms_opt(0, 0, 0).unwrap()))
}

fn print_report(s: &SessionState) {
    let n = s.trades.len();
    let wins = s.trades.iter().filter(|t| t.pnl > 0.0).count();
    let pnl: f64 = s.trades.iter().map(|t| t.pnl).sum();
    println!("\n───── BACKTEST RESULTS ─────");
    println!("Trades:       {n}");
    println!("Missed trades:{:>4}", s.missed_trades);
    println!("Win rate:     {:.1}%",
             if n == 0 { 0.0 } else { wins as f64 / n as f64 * 100.0 });
    println!("Total PnL:    ${pnl:.2}");
    println!("Final equity: ${:.2}", s.equity);
    println!("Return:       {:.2}%",
             (s.equity / s.cfg.initial_equity - 1.0) * 100.0);

    let st = stats::analyze(&s.trades, s.cfg.bars_per_year);
    println!("\n───── STATISTICAL SIGNIFICANCE ─────");
    println!("Sample size:  {}", st.n_trades);
    println!("Win rate:     {:.1}%  (95% CI {:.1}%–{:.1}%)",
             st.win_rate * 100.0, st.win_rate_ci.0 * 100.0, st.win_rate_ci.1 * 100.0);
    println!("Break-even:   {:.1}%", st.breakeven_wr * 100.0);
    println!("Significant:  {}", if st.is_significant { "YES" } else { "NO" });
    println!("Total PnL CI: [${:.2}, ${:.2}]", st.pnl_ci.0, st.pnl_ci.1);
    println!("Sharpe:       {:.2}  (95% CI {:.2}–{:.2})",
             st.sharpe, st.sharpe_ci.0, st.sharpe_ci.1);

    if !st.warnings.is_empty() {
        println!("\n───── WARNINGS ─────");
        for w in &st.warnings { println!("{w}"); }
        println!("\n🚫 DO NOT TRADE LIVE until significance warnings clear.");
    }
}

async fn run_point_prediction_diagnostic(
    bars: &[Bar],
    cfg: &Config,
    pcfg: point_predictor::PointPredictorConfig,
) -> Result<()> {
    let mut warm = SessionState::new(cfg.clone());
    let mut evals = Vec::with_capacity(bars.len());
    for b in bars {
        warm.step(b);
        evals.push(warm.last_eval.clone().unwrap_or_else(default_eval));
    }
    let atrs: Vec<f64> = evals.iter().filter_map(|e| e.atr).collect();
    let avg_atr = if atrs.is_empty() { 1.0 } else { atrs.iter().sum::<f64>() / atrs.len() as f64 };
    let typical_sl = cfg.atr_sl_mult * avg_atr;
    let pp = point_predictor::PointPredictor::train(
        bars, &evals, pcfg, cfg.spread, cfg.slippage, typical_sl,
    )?;
    point_predictor::print_diagnostic(&pp, cfg.spread, cfg.slippage, typical_sl);
    Ok(())
}

async fn run_live_trading(
    broker: IgBroker,
    cfg: Config,
    trained: Option<CalibratedPredictor>,
) -> Result<()> {
    let demo = std::env::var("IG_BASE_URL")
        .map(|u| u.contains("demo"))
        .unwrap_or(true);
    println!("▶ IG live loop ({}) — epic {}",
             if demo { "DEMO" } else { "🔴 LIVE MONEY" }, broker.epic());

    let warm_end = Utc::now();
    let warm_start = warm_end - chrono::Duration::minutes(200);
    let warm = data::fetch_ig_bars(&broker, "MINUTE", warm_start, warm_end).await?;
    let mut state = make_session(cfg.clone(), trained);
    for b in &warm { state.step(b); }
    println!("Primed with {} bars", warm.len());

    let mut cur_minute: Option<i64> = None;
    let mut bar_high = f64::MIN;
    let mut bar_low = f64::MAX;
    let mut bar_open = 0.0;
    let mut last_price = 0.0;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));

    println!("▶ Polling IG snapshot every 2s. Ctrl-C to stop.");

    loop {
        tick.tick().await;
        let (bid, offer) = match broker.snapshot().await {
            Ok(v) => v,
            Err(e) => { eprintln!("snapshot error: {e}"); continue; }
        };
        let mid = (bid + offer) / 2.0;
        let now = Utc::now().timestamp();
        let minute = now - now.rem_euclid(60);

        if cur_minute != Some(minute) {
            if cur_minute.is_some() && bar_open > 0.0 {
                let bar = Bar {
                    ts: cur_minute.unwrap(), open: bar_open,
                    high: bar_high, low: bar_low, close: last_price, volume: 1.0,
                };
                state.step(&bar);
                if let Some(p) = state.pending_signal.take() {
                    let (sl, tp) = match p.sig.side {
                        Side::Long => (mid - cfg.atr_sl_mult * p.atr_at_signal,
                                       mid + cfg.atr_tp_mult * p.atr_at_signal),
                        Side::Short => (mid + cfg.atr_sl_mult * p.atr_at_signal,
                                        mid - cfg.atr_tp_mult * p.atr_at_signal),
                    };
                    match broker.place_market_order(p.sig.side, broker.size(), Some(sl), Some(tp)).await {
                        Ok(ref_id) => println!("✅ {} {} {:.2} @ {:.2} — deal ref {}",
                            p.sig.side.as_str(), p.sig.kind.as_str(), broker.size(), mid, ref_id),
                        Err(e) => eprintln!("❌ order failed: {e}"),
                    }
                }
            }
            cur_minute = Some(minute);
            bar_open = mid; bar_high = mid; bar_low = mid; last_price = mid;
        } else {
            bar_high = bar_high.max(mid);
            bar_low = bar_low.min(mid);
            last_price = mid;
        }
    }
}

// ── news sources ───────────────────────────────────────────────────

const FED_FOMC_CALENDAR: &str =
    "https://www.federalreserve.gov/monetarypolicy/fomccalendars.htm";

const GOLD_RSS_FEEDS: &[(&str, &str)] = &[
    (
        "Google News (gold)",
        "https://news.google.com/rss/search?q=gold+price+OR+XAUUSD+OR+%22gold+futures%22&hl=en-US&gl=US&ceid=US:en",
    ),
    (
        "Yahoo Finance (GC=F)",
        "https://feeds.finance.yahoo.com/rss/2.0/headline?s=GC%3DF&region=US&lang=en-US",
    ),
];

async fn fetch_news_events(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<NewsEvent>> {
    let mut events: Vec<NewsEvent> = Vec::new();

    let fomc = fomc_dates().await;
    let mut fomc_added = 0;
    for ts in &fomc {
        if *ts >= start.timestamp() && *ts <= end.timestamp() {
            let when = chrono::DateTime::from_timestamp(*ts, 0)
                .map(|d| d.format("%b %d, %Y").to_string())
                .unwrap_or_default();
            events.push(NewsEvent {
                ts: *ts,
                kind: NewsKind::Fomc,
                title: format!("FOMC meeting — {}", when),
            });
            fomc_added += 1;
        }
    }
    if fomc_added > 0 {
        println!("  FOMC: {} meetings in range", fomc_added);
    }

    match fetch_gold_news().await {
        Ok(news) => {
            let mut added = 0;
            for (ts, title) in news {
                if !is_gold_relevant(&title) { continue; }
                if ts < start.timestamp() || ts > end.timestamp() { continue; }
                events.push(NewsEvent {
                    ts,
                    kind: NewsKind::News,
                    title,
                });
                added += 1;
            }
            if added > 0 {
                println!("  News: {} gold-related headlines", added);
            }
        }
        Err(e) => eprintln!("  ⚠️  news fetch failed: {e}"),
    }

    events.sort_by_key(|e| e.ts);
    events.dedup_by(|a, b| a.ts == b.ts && a.title == b.title);
    Ok(events)
}

async fn fetch_gold_news() -> Result<Vec<(i64, String)>> {
    let mut all: Vec<(i64, String)> = Vec::new();
    let mut any_ok = false;
    let mut last_err = String::new();

    for (name, url) in GOLD_RSS_FEEDS {
        match fetch_rss_items(url).await {
            Ok(items) => {
                any_ok = true;
                if !items.is_empty() {
                    println!("  {}: {} headlines", name, items.len());
                }
                all.extend(items);
            }
            Err(e) => {
                eprintln!("  ⚠️  {} failed: {}", name, e);
                last_err = e.to_string();
            }
        }
    }

    if !any_ok && !last_err.is_empty() {
        anyhow::bail!("all RSS feeds failed (last: {})", last_err);
    }
    all.sort_by_key(|(ts, _)| *ts);
    all.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    Ok(all)
}

async fn fetch_rss_items(url: &str) -> Result<Vec<(i64, String)>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (compatible; Aurifer/0.1)")
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let text = resp.text().await?;

    let item_re  = Regex::new(r"(?s)<item>(.*?)</item>")?;
    let title_re = Regex::new(r"(?s)<title>(.*?)</title>")?;
    let date_re  = Regex::new(r"(?s)<pubDate>(.*?)</pubDate>")?;

    let mut out = Vec::new();
    for cap in item_re.captures_iter(&text) {
        let block = &cap[1];
        let title = title_re
            .captures(block)
            .map(|c| decode_xml(strip_cdata(&c[1])))
            .unwrap_or_default();
        if title.is_empty() { continue; }
        let title = truncate_title(&title, 140);
        let date_str = date_re
            .captures(block)
            .map(|c| strip_cdata(&c[1]).to_string())
            .unwrap_or_default();
        if let Some(ts) = parse_rss_date(&date_str) {
            out.push((ts, title));
        }
    }
    Ok(out)
}

fn strip_cdata(s: &str) -> &str {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").unwrap_or(s);
    let s = s.strip_suffix("]]>").unwrap_or(s);
    s.trim()
}

fn parse_rss_date(s: &str) -> Option<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
        return Some(dt.timestamp());
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    for fmt in &[
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%a, %d %b %Y %H:%M:%S",
    ] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc().timestamp());
        }
    }
    None
}

fn decode_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

fn truncate_title(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max { return s.to_string(); }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

async fn fomc_dates() -> Vec<i64> {
    let mut all: Vec<i64> = fallback_fomc_dates();

    match scrape_fomc_dates().await {
        Ok(dates) if !dates.is_empty() => {
            println!(
                "  Fed calendar: {} scraped + {} fallback dates",
                dates.len(),
                all.len()
            );
            all.extend(dates);
        }
        Ok(_) => eprintln!("  ⚠️  Fed calendar returned 0 dates — using fallback only"),
        Err(e) => eprintln!("  ⚠️  Fed calendar fetch failed: {e} — using fallback"),
    }

    all.sort();
    all.dedup();
    all
}

async fn scrape_fomc_dates() -> Result<Vec<i64>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (compatible; Aurifer/0.1)")
        .build()?;

    let html = client.get(FED_FOMC_CALENDAR).send().await?.text().await?;

    let re = Regex::new(r"fomcminutes(\d{4})(\d{2})(\d{2})\.(?:pdf|htm)")?;

    let mut out: Vec<i64> = Vec::new();
    for cap in re.captures_iter(&html) {
        let year: i32 = cap[1].parse().unwrap_or(0);
        let month: u32 = cap[2].parse().unwrap_or(0);
        let day: u32 = cap[3].parse().unwrap_or(0);
        if year == 0 || month == 0 || day == 0 { continue; }

        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            if let Some(dt) = date.and_hms_opt(18, 0, 0) {
                out.push(dt.and_utc().timestamp());
            }
        }
    }

    out.sort();
    out.dedup();
    Ok(out)
}

fn fallback_fomc_dates() -> Vec<i64> {
    let dates: &[(i32, u32, u32)] = &[
        (2014, 1, 29), (2014, 3, 19), (2014, 4, 30), (2014, 6, 18),
        (2014, 7, 30), (2014, 9, 17), (2014, 10, 29), (2014, 12, 17),
        (2015, 1, 28), (2015, 3, 18), (2015, 4, 29), (2015, 6, 17),
        (2015, 7, 29), (2015, 9, 17), (2015, 10, 28), (2015, 12, 16),
        (2016, 1, 27), (2016, 3, 16), (2016, 4, 27), (2016, 6, 15),
        (2016, 7, 27), (2016, 9, 21), (2016, 11, 2), (2016, 12, 14),
        (2017, 2, 1), (2017, 3, 15), (2017, 5, 3), (2017, 6, 14),
        (2017, 7, 26), (2017, 9, 20), (2017, 11, 1), (2017, 12, 13),
        (2018, 1, 31), (2018, 3, 21), (2018, 5, 2), (2018, 6, 13),
        (2018, 8, 1), (2018, 9, 26), (2018, 11, 8), (2018, 12, 19),
        (2019, 1, 30), (2019, 3, 20), (2019, 5, 1), (2019, 6, 19),
        (2019, 7, 31), (2019, 9, 18), (2019, 10, 30), (2019, 12, 11),
        (2020, 1, 29), (2020, 3, 3), (2020, 3, 15), (2020, 4, 29),
        (2020, 6, 10), (2020, 7, 29), (2020, 9, 16), (2020, 11, 5),
        (2020, 12, 16),
        (2021, 1, 27), (2021, 3, 17), (2021, 4, 28), (2021, 6, 16),
        (2021, 7, 28), (2021, 9, 22), (2021, 11, 3), (2021, 12, 15),
        (2022, 1, 26), (2022, 3, 16), (2022, 5, 4), (2022, 6, 15),
        (2022, 7, 27), (2022, 9, 21), (2022, 11, 2), (2022, 12, 14),
        (2023, 2, 1), (2023, 3, 22), (2023, 5, 3), (2023, 6, 14),
        (2023, 7, 26), (2023, 9, 20), (2023, 11, 1), (2023, 12, 13),
        (2024, 1, 31), (2024, 3, 20), (2024, 5, 1), (2024, 6, 12),
        (2024, 7, 31), (2024, 9, 18), (2024, 11, 7), (2024, 12, 18),
        (2025, 1, 29), (2025, 3, 19), (2025, 5, 7), (2025, 6, 18),
        (2025, 7, 30), (2025, 9, 17), (2025, 10, 29), (2025, 12, 10),
        (2026, 1, 28), (2026, 3, 18), (2026, 4, 29), (2026, 6, 17),
        (2026, 7, 29), (2026, 9, 16), (2026, 11, 4), (2026, 12, 16),
    ];

    dates.iter().filter_map(|(y, m, d)| {
        NaiveDate::from_ymd_opt(*y, *m, *d)
            .and_then(|dt| dt.and_hms_opt(18, 0, 0))
            .map(|dt| dt.and_utc().timestamp())
    }).collect()
}

fn is_gold_relevant(text: &str) -> bool {
    let s = text.to_lowercase();
    matches!(
        s.as_str(),
        "non_farm_payrolls" | "nonfarm_payrolls" | "nfp" | "inflation" | "cpi"
            | "core_inflation" | "core_cpi" | "policy_rate" | "interest_rate" | "fomc"
            | "fed_funds_rate" | "producer_price_index" | "ppi" | "gdp" | "gdp_growth"
            | "retail_sales" | "unemployment" | "unemployment_rate" | "core_pce" | "pce"
            | "ism_manufacturing" | "ism_services" | "jobless_claims" | "initial_jobless_claims"
            | "average_hourly_earnings"
    )
        || s.contains("gold")
        || s.contains("xau")
        || s.contains("bullion")
        || s.contains("payroll")
        || s.contains("cpi")
        || s.contains("inflation")
        || s.contains("fomc")
        || s.contains("fed ")
        || s.contains("federal reserve")
}