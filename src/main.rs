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
    let start = parse_date(&args.start)?;
    let end = parse_date(&args.end)?;

    let ig: Option<IgBroker> = if args.no_ig {
        None
    } else {
        match IgBroker::connect().await {
            Ok(b) => { println!("✅ IG connected — epic {}", b.epic()); Some(b) }
            Err(e) => {
                eprintln!("⚠️ IG connection failed: {e}");
                eprintln!("⚠️ Falling back to synthetic data. Set IG_* in .env to use IG.");
                None
            }
        }
    };

    let bars: Vec<Bar> = match &ig {
        Some(b) => {
            println!("Fetching gold bars from IG ({} → {}) …",
                     start.format("%Y-%m-%d"), end.format("%Y-%m-%d"));
            let n = ((end - start).num_minutes()).clamp(100, 1000) as usize;
            data::fetch_ig_bars(b, "MINUTE", n).await?
        }
        None => data::fetch_synthetic_bars(start, end).await?,
    };
    println!("Loaded {} 1-min bars", bars.len());
    if bars.is_empty() { anyhow::bail!("no bars returned"); }

    let mut cfg = Config::default();
    cfg.spread = 0.30;
    cfg.commission_per_unit = 0.0;
    cfg.data_source_is_mid_price = ig.is_none();
    let mut warnings = Vec::new();
    if cfg.data_source_is_mid_price {
        warnings.push("synthetic data — all results meaningless".into());
    }
    if !args.no_news {
        if let Ok(key) = std::env::var("QUANTGIST_API_KEY") {
            match fetch_news(&key, 180).await {
                Ok(w) if !w.is_empty() => cfg.news_blackouts = w,
                Ok(_) => warnings.push("news calendar returned 0 events".into()),
                Err(e) => warnings.push(format!("news fetch failed: {e}")),
            }
        } else {
            warnings.push("QUANTGIST_API_KEY unset".into());
        }
    }

    let trained: Option<CalibratedPredictor> = if args.no_predictor {
        None
    } else {
        match train_and_validate(&bars, &cfg) {
            Ok(t) => t,
            Err(e) => { eprintln!("⚠️ predictor disabled: {e}"); None }
        }
    };
    if trained.is_some() { warnings.push("predictor enabled".into()); }

    match args.command.unwrap_or(Command::Tui) {
        Command::Backtest => {
            let mut s = make_session(cfg, trained);
            for b in &bars { s.step(b); }
            if let Some(l) = bars.last() { s.force_close(l); }
            print_report(&s);
        }
        Command::Tui => {
            let mut app = tui::App::new(bars, cfg, warnings, trained);
            app.run().context("TUI error")?;
        }
        Command::Web => {
            web::run_web_server(bars, cfg, warnings, trained, !args.no_open).await?;
        }
        Command::Live => {
            if !args.confirm_live {
                anyhow::bail!("Live mode requires --confirm-live. This places REAL orders.");
            }
            let b = ig.context("live mode requires IG credentials in .env")?;
            run_live_trading(b, cfg, trained).await?;
        }
        Command::Diagnostics { sub: DiagnosticsCommand::PointPrediction } => {
            let mut pcfg = point_predictor::PointPredictorConfig::default();
            pcfg.horizon = args.horizon;
            run_point_prediction_diagnostic(&bars, &cfg, pcfg).await?;
        }
    }
    Ok(())
}

async fn run_live_trading(broker: IgBroker, cfg: Config, trained: Option<CalibratedPredictor>) -> Result<()> {
    let demo = std::env::var("IG_BASE_URL").map(|u| u.contains("demo")).unwrap_or(true);
    println!("▶ IG live loop ({}) — epic {}",
             if demo { "DEMO" } else { "🔴 LIVE MONEY" }, broker.epic());

    let warm = data::fetch_ig_bars(&broker, "MINUTE", 200).await?;
    let mut state = make_session(cfg.clone(), trained);
    for b in &warm { state.step(b); }
    println!("Primed with {} bars", warm.len());

    let mut cur_minute: Option<i64> = None;
    let mut bar_high = f64::MIN; let mut bar_low = f64::MAX;
    let mut bar_open = 0.0; let mut last_price = 0.0;
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
                let bar = Bar { ts: cur_minute.unwrap(), open: bar_open,
                                high: bar_high, low: bar_low, close: last_price, volume: 1.0 };
                state.step(&bar);
                if let Some(p) = state.pending_signal.take() {
                    let (sl, tp) = match p.sig.side {
                        Side::Long  => (mid - cfg.atr_sl_mult * p.atr_at_signal,
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

fn make_session(cfg: Config, trained: Option<CalibratedPredictor>) -> SessionState {
    match trained { Some(t) => SessionState::with_predictor(cfg, t), None => SessionState::new(cfg) }
}

fn train_and_validate(bars: &[Bar], cfg: &Config) -> Result<Option<CalibratedPredictor>> {
    let mut warm = SessionState::new(cfg.clone());
    let mut evals = Vec::with_capacity(bars.len());
    for b in bars { warm.step(b); evals.push(warm.last_eval.clone().unwrap_or_else(default_eval)); }
    let pcfg = PredictorConfig::default();
    println!("\n── Walk-forward validation ──");
    let wf = predictor::walk_forward_validate(bars, &evals, &pcfg, 4)?;
    if let Err(e) = wf.validate() {
        eprintln!("🚫 Walk-forward validation failed: {e}");
        return Ok(None);
    }
    println!("\n── Full predictor training ──");
    Ok(Some(CalibratedPredictor::train_calibrated(bars, &evals, pcfg)?))
}

fn default_eval() -> strategies::Eval {
    strategies::Eval { bar_index: 0, atr: None, adx: None, rsi: None,
        trend_up: false, trend_ready: false, squeeze_ok: false,
        flow: None, signals: vec![], prediction: None }
}

fn parse_date(s: &str) -> Result<DateTime<Utc>> {
    let n = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("bad date: {s}"))?;
    Ok(Utc.from_utc_datetime(&n.and_hms_opt(0, 0, 0).unwrap()))
}

fn print_report(s: &SessionState) {
    let n = s.trades.len();
    let wins = s.trades.iter().filter(|t| t.pnl > 0.0).count();
    let pnl: f64 = s.trades.iter().map(|t| t.pnl).sum();
    println!("\n───── BACKTEST RESULTS ─────");
    println!("Trades:       {n}");
    println!("Missed trades:{:>4}", s.missed_trades);
    println!("Win rate:     {:.1}%", if n == 0 { 0.0 } else { wins as f64 / n as f64 * 100.0 });
    println!("Total PnL:    ${pnl:.2}");
    println!("Final equity: ${:.2}", s.equity);
    println!("Return:       {:.2}%", (s.equity / s.cfg.initial_equity - 1.0) * 100.0);

    if s.realized_slippage_n > 0 {
        let avg = s.realized_slippage_sum / s.realized_slippage_n as f64;
        println!("\nRealized adverse gap: {:.4} (n={})", avg, s.realized_slippage_n);
        println!("Modeled slippage:     {:.4}", s.cfg.slippage);
        if avg > s.cfg.slippage * 1.5 { println!("⚠️ Realized gap >1.5× modeled."); }
    }

    let st = stats::analyze(&s.trades, s.cfg.bars_per_year);
    println!("\n───── STATISTICAL SIGNIFICANCE ─────");
    println!("Sample size:  {}", st.n_trades);
    println!("Win rate:     {:.1}%  (95% CI {:.1}%–{:.1}%)",
             st.win_rate * 100.0, st.win_rate_ci.0 * 100.0, st.win_rate_ci.1 * 100.0);
    println!("Break-even:   {:.1}%", st.breakeven_wr * 100.0);
    println!("Significant:  {}", if st.is_significant { "YES" } else { "NO" });
    println!("Total PnL CI: [${:.2}, ${:.2}]", st.pnl_ci.0, st.pnl_ci.1);
    println!("Sharpe:       {:.2}  (95% CI {:.2}–{:.2})", st.sharpe, st.sharpe_ci.0, st.sharpe_ci.1);

    if !st.warnings.is_empty() {
        println!("\n───── WARNINGS ─────");
        for w in &st.warnings { println!("{w}"); }
        println!("\n🚫 DO NOT TRADE LIVE until significance warnings clear.");
    }
}

async fn run_point_prediction_diagnostic(bars: &[Bar], cfg: &Config, pcfg: point_predictor::PointPredictorConfig) -> Result<()> {
    let mut warm = SessionState::new(cfg.clone());
    let mut evals = Vec::with_capacity(bars.len());
    for b in bars { warm.step(b); evals.push(warm.last_eval.clone().unwrap_or_else(default_eval)); }
    let atrs: Vec<f64> = evals.iter().filter_map(|e| e.atr).collect();
    let avg_atr = if atrs.is_empty() { 1.0 } else { atrs.iter().sum::<f64>() / atrs.len() as f64 };
    let typical_sl = cfg.atr_sl_mult * avg_atr;
    let pp = point_predictor::PointPredictor::train(bars, &evals, pcfg, cfg.spread, cfg.slippage, typical_sl)?;
    point_predictor::print_diagnostic(&pp, cfg.spread, cfg.slippage, typical_sl);
    Ok(())
}

async fn fetch_news(key: &str, days: u32) -> Result<Vec<(i64, i64)>> {
    let url = format!("https://api.quantgist.com/v1/macro/calendar?events=CPI,NFP,FOMC&days={days}");
    let r: serde_json::Value = reqwest::Client::new()
        .get(&url).header("X-API-Key", key).send().await?.json().await?;
    let mut w = Vec::new();
    if let Some(events) = r["events"].as_array() {
        for ev in events {
            if let Some(t) = ev["scheduled_at"].as_str() {
                let ts = DateTime::parse_from_rfc3339(t)?.timestamp();
                w.push((ts - 15 * 60, ts + 15 * 60));
            }
        }
    }
    Ok(w)
}