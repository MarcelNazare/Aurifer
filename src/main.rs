#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;

mod broker;
mod cli;
mod costs;
mod data;
mod indicators;
mod point_predictor;
mod predictor;
mod session;
mod stats;
mod strategies;
mod tui;
mod types;
mod web;

use broker::IgBroker;
use cli::{Cli, Command, DiagnosticsCommand};
use predictor::{CalibratedPredictor, PredictorConfig};
use session::SessionState;
use types::*;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let args = Cli::parse();
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
                eprintln!("⚠️ Falling back to synthetic data. Set IG_* in .env to use IG.");
                None
            }
        }
    };

    match args.command.as_ref().unwrap_or(&Command::Tui) {
        Command::Forecast => {
            run_forecast(ig.as_ref(), &args, start, end).await?;
        }
        Command::Backtest => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let cfg = base_config(ig.is_none(), &args);
            let trained = try_train(&bars, &cfg, &args)?;
            let mut s = make_session(cfg.clone(), trained);
            for b in &bars { s.step(b); }
            if let Some(l) = bars.last() { s.force_close(l); }
            print_report(&s);
        }
        Command::Tui => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let cfg = base_config(ig.is_none(), &args);
            let warnings = build_warnings(&cfg, &args).await;
            let trained = try_train(&bars, &cfg, &args)?;
            let mut app = tui::App::new(bars, cfg, warnings, trained);
            app.run().context("TUI error")?;
        }
        Command::Web => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let cfg = base_config(ig.is_none(), &args);
            let warnings = build_warnings(&cfg, &args).await;
            let trained = try_train(&bars, &cfg, &args)?;
            web::run_web_server(bars, cfg, warnings, trained, !args.no_open).await?;
        }
        Command::Live => {
            if !args.confirm_live {
                anyhow::bail!("Live mode requires --confirm-live. This places REAL orders.");
            }
            let b = ig.context("live mode requires IG credentials in .env")?;
            let cfg = base_config(false, &args);
            let bars = load_bars(Some(&b), &start, &end, &args).await?;
            let trained = try_train(&bars, &cfg, &args)?;
            run_live_trading(b, cfg, trained).await?;
        }
        Command::Diagnostics { sub: DiagnosticsCommand::PointPrediction } => {
            let bars = load_bars(ig.as_ref(), &start, &end, &args).await?;
            let cfg = base_config(ig.is_none(), &args);
            let mut pcfg = point_predictor::PointPredictorConfig::default();
            pcfg.horizon = args.horizon;
            run_point_prediction_diagnostic(&bars, &cfg, pcfg).await?;
        }
    }
    Ok(())
}

// ── Forecast ────────────────────────────────────────────────────────

async fn run_forecast(
    ig: Option<&IgBroker>,
    args: &Cli,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    let broker = ig.context("forecast requires IG credentials — check .env")?;

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

    let bars = data::fetch_ig_bars(broker, &args.resolution, start, end).await?;
    println!("  Loaded:     {} bars", bars.len());

    if bars.len() < 2500 {
        anyhow::bail!(
            "only {} bars available — need at least 2,500. \
             Extend the window: aurifer forecast --start 2015-01-01",
            bars.len()
        );
    }

    let cfg = Config::default();
    let mut warm = SessionState::new(cfg.clone());
    let mut evals = Vec::with_capacity(bars.len());
    for b in &bars {
        warm.step(b);
        evals.push(warm.last_eval.clone().unwrap_or_else(default_eval));
    }

    let mut pcfg = PredictorConfig::default();
    pcfg.horizon = args.horizon;
    pcfg.label_atr_mult = args.label_atr_mult;

    println!();
    println!("  Validating model (walk-forward) …");
    let wf_result = predictor::walk_forward_validate(&bars, &evals, &pcfg, 4);
    let validated = match &wf_result {
        Ok(r) => r.validate().is_ok(),
        Err(e) => { println!("  walk-forward error: {e}"); false }
    };

    let trained = CalibratedPredictor::train_calibrated(&bars, &evals, pcfg.clone())
        .context("predictor training failed")?;

    let base_rate = trained.metrics.base_rate;
    let base_rate_ok = (0.30..=0.70).contains(&base_rate);

    let mut engine = strategies::StrategyEngine::with_predictor(cfg, trained.clone());
    let mut last_eval = None;
    for b in &bars { last_eval = Some(engine.on_bar(b)); }
    let eval = last_eval.context("engine produced no eval")?;
    let p_up = eval.prediction.context("predictor produced no probability")?;

    let last_bar = bars.last().unwrap();
    let last_date = chrono::DateTime::from_timestamp(last_bar.ts, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default();

    let direction = if p_up >= 0.60 { "LIKELY UP" }
        else if p_up <= 0.40 { "LIKELY DOWN" }
        else { "UNCERTAIN" };
    let confidence = if (p_up - 0.5).abs() > 0.15 { "HIGH" }
        else if (p_up - 0.5).abs() > 0.05 { "MEDIUM" }
        else { "LOW" };
    let edge = (p_up - 0.5).abs() * 100.0;

    println!();
    println!("───────────────────────────────────────────────────────────");
    println!("  As of:          {}  (last close)", last_date);
    println!("  Current price:  {:.2}", last_bar.close);
    println!("  ATR(14):        {:.2}", eval.atr.unwrap_or(0.0));
    println!("  RSI(14):        {:.1}", eval.rsi.unwrap_or(0.0));
    println!("  ADX(14):        {:.1}", eval.adx.unwrap_or(0.0));
    println!();
    println!(
        "  ─── Prediction: next {} {} bar{} ───",
        args.horizon,
        args.resolution,
        if args.horizon == 1 { "" } else { "s" }
    );
    println!("  P(up):          {:.1}%", p_up * 100.0);
    println!("  P(not up):      {:.1}%  (down or flat)", (1.0 - p_up) * 100.0);
    println!("  Direction:      {}", direction);
    println!("  Confidence:     {}  (edge: {:.1}% above 50%)", confidence, edge);
    println!();
    println!("  ─── Model quality ───");
    if validated {
        println!("  Walk-forward:   ✅ VALIDATED");
    } else {
        println!("  Walk-forward:   ⚠️  UNVALIDATED");
        println!("  ⚠️  The model has NOT passed walk-forward validation.");
        println!("  ⚠️  This forecast is for research only — do not trade it.");
    }
    println!(
        "  AUC:            {:.3}  (95% CI {:.3}–{:.3})",
        trained.metrics.auc, trained.auc_ci_95.0, trained.auc_ci_95.1
    );
    println!(
        "  Brier:          {:.4}  ({})",
        trained.brier_calibrated,
        if trained.brier_calibrated < 0.24 { "good" } else { "poor" }
    );
    println!(
        "  Base rate:      {:.1}%  ({})",
        base_rate * 100.0,
        if base_rate_ok { "balanced" } else { "IMBALANCED" }
    );
    println!();

    if !base_rate_ok {
        println!("  ⚠️  LABEL IMBALANCE: base rate {:.1}% (target: 30–70%)", base_rate * 100.0);
        if base_rate < 0.30 {
            println!("  ⚠️  Too few positives — reduce --label-atr-mult (try 0.15)");
        } else {
            println!("  ⚠️  Too many positives — increase --label-atr-mult (try 0.40)");
        }
        println!();
    }
    println!("═══════════════════════════════════════════════════════════");
    Ok(())
}

// ── Shared helpers ──────────────────────────────────────────────────

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

    let bars = match ig {
        Some(b) => {
            println!(
                "Fetching gold bars from IG ({} → {}, resolution={}) …",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d"),
                args.resolution
            );
            data::fetch_ig_bars(b, &args.resolution, *start, *end).await?
        }
        None => data::fetch_synthetic_bars(*start, *end).await?,
    };
    println!("Loaded {} {} bars", bars.len(), args.resolution);
    if bars.is_empty() {
        anyhow::bail!("no bars returned. Try a recent date range.");
    }
    Ok(bars)
}

fn base_config(mid_price: bool, args: &Cli) -> Config {
    let mut cfg = Config::default();
    apply_resolution_defaults(&mut cfg, &args.resolution);
    cfg.spread = 0.30;
    cfg.commission_per_unit = 0.0;
    cfg.data_source_is_mid_price = mid_price;
    cfg
}

async fn build_warnings(_cfg: &Config, args: &Cli) -> Vec<String> {
    let mut warnings = Vec::new();
    if _cfg.data_source_is_mid_price {
        warnings.push("synthetic data — all results meaningless".into());
    }
    if args.resolution.to_uppercase() != "MINUTE" {
        warnings.push(format!(
            "resolution={} — live loop runs on MINUTE bars. backtest validates the strategy idea, not the live execution.",
            args.resolution
        ));
    }
    if !args.no_news && std::env::var("QUANTGIST_API_KEY").is_err() {
        warnings.push("QUANTGIST_API_KEY unset — trading through news events".into());
    }
    warnings
}

fn try_train(bars: &[Bar], cfg: &Config, args: &Cli) -> Result<Option<CalibratedPredictor>> {
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
    if let Err(e) = wf.validate() {
        eprintln!("🚫 Walk-forward validation failed: {e}");
        return Ok(None);
    }
    println!("\n── Full predictor training ──");
    Ok(Some(CalibratedPredictor::train_calibrated(bars, &evals, pcfg)?))
}

fn resolve_date_range(start_str: &str, end_str: &str) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = if end_str.is_empty() {
        Utc::now()
    } else {
        parse_date(end_str)?
    };
    let start = if start_str.is_empty() {
        end - chrono::Duration::days(90)
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
    println!(
        "Win rate:     {:.1}%",
        if n == 0 { 0.0 } else { wins as f64 / n as f64 * 100.0 }
    );
    println!("Total PnL:    ${pnl:.2}");
    println!("Final equity: ${:.2}", s.equity);
    println!("Return:       {:.2}%", (s.equity / s.cfg.initial_equity - 1.0) * 100.0);

    let st = stats::analyze(&s.trades, s.cfg.bars_per_year);
    println!("\n───── STATISTICAL SIGNIFICANCE ─────");
    println!("Sample size:  {}", st.n_trades);
    println!(
        "Win rate:     {:.1}%  (95% CI {:.1}%–{:.1}%)",
        st.win_rate * 100.0,
        st.win_rate_ci.0 * 100.0,
        st.win_rate_ci.1 * 100.0
    );
    println!("Break-even:   {:.1}%", st.breakeven_wr * 100.0);
    println!("Significant:  {}", if st.is_significant { "YES" } else { "NO" });
    println!("Total PnL CI: [${:.2}, ${:.2}]", st.pnl_ci.0, st.pnl_ci.1);
    println!(
        "Sharpe:       {:.2}  (95% CI {:.2}–{:.2})",
        st.sharpe, st.sharpe_ci.0, st.sharpe_ci.1
    );

    if !st.warnings.is_empty() {
        println!("\n───── WARNINGS ─────");
        for w in &st.warnings {
            println!("{w}");
        }
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
    let avg_atr = if atrs.is_empty() {
        1.0
    } else {
        atrs.iter().sum::<f64>() / atrs.len() as f64
    };
    let typical_sl = cfg.atr_sl_mult * avg_atr;
    let pp = point_predictor::PointPredictor::train(
        bars,
        &evals,
        pcfg,
        cfg.spread,
        cfg.slippage,
        typical_sl,
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
    println!(
        "▶ IG live loop ({}) — epic {}",
        if demo { "DEMO" } else { "🔴 LIVE MONEY" },
        broker.epic()
    );
    println!("⚠️ Live loop builds 1-minute bars from 2s snapshots.");

    let warm_end = Utc::now();
    let warm_start = warm_end - chrono::Duration::minutes(200);
    let warm = data::fetch_ig_bars(&broker, "MINUTE", warm_start, warm_end).await?;
    let mut state = make_session(cfg.clone(), trained);
    for b in &warm {
        state.step(b);
    }
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
            Err(e) => {
                eprintln!("snapshot error: {e}");
                continue;
            }
        };
        let mid = (bid + offer) / 2.0;
        let now = Utc::now().timestamp();
        let minute = now - now.rem_euclid(60);

        if cur_minute != Some(minute) {
            if cur_minute.is_some() && bar_open > 0.0 {
                let bar = Bar {
                    ts: cur_minute.unwrap(),
                    open: bar_open,
                    high: bar_high,
                    low: bar_low,
                    close: last_price,
                    volume: 1.0,
                };
                state.step(&bar);
                if let Some(p) = state.pending_signal.take() {
                    let (sl, tp) = match p.sig.side {
                        Side::Long => (
                            mid - cfg.atr_sl_mult * p.atr_at_signal,
                            mid + cfg.atr_tp_mult * p.atr_at_signal,
                        ),
                        Side::Short => (
                            mid + cfg.atr_sl_mult * p.atr_at_signal,
                            mid - cfg.atr_tp_mult * p.atr_at_signal,
                        ),
                    };
                    match broker
                        .place_market_order(p.sig.side, broker.size(), Some(sl), Some(tp))
                        .await
                    {
                        Ok(ref_id) => println!(
                            "✅ {} {} {:.2} @ {:.2} — deal ref {}",
                            p.sig.side.as_str(),
                            p.sig.kind.as_str(),
                            broker.size(),
                            mid,
                            ref_id
                        ),
                        Err(e) => eprintln!("❌ order failed: {e}"),
                    }
                }
            }
            cur_minute = Some(minute);
            bar_open = mid;
            bar_high = mid;
            bar_low = mid;
            last_price = mid;
        } else {
            bar_high = bar_high.max(mid);
            bar_low = bar_low.min(mid);
            last_price = mid;
        }
    }
}