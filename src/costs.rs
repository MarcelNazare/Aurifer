use crate::types::Config;
use chrono::{DateTime, Timelike, Utc};

pub fn session_spread_mult(ts: i64) -> f64 {
    let dt = DateTime::<Utc>::from_timestamp(ts, 0).unwrap_or_default();
    match dt.hour() { 22 | 23 | 0 | 1 => 2.0, 2..=6 => 1.6, 7..=16 => 1.0, 17..=21 => 1.4, _ => 1.5 }
}
pub fn vol_spread_mult(atr: f64) -> f64 { (1.0 + (atr / 2.0).min(1.5)).min(2.5) }
pub fn effective_spread(cfg: &Config, ts: i64, atr: f64) -> f64 {
    (cfg.spread * session_spread_mult(ts) * vol_spread_mult(atr)).min(cfg.max_spread * 2.0)
}
pub fn effective_slippage(cfg: &Config, units: f64, equity: f64, atr: f64) -> f64 {
    let baseline_risk = equity * cfg.risk_per_trade;
    let baseline_units = if atr > 0.0 { baseline_risk / (cfg.atr_sl_mult * atr) } else { 1.0 };
    let ratio = (units / baseline_units.max(0.01)).max(0.1);
    cfg.slippage * ratio.sqrt().min(3.0)
}
pub struct FillCosts { pub spread_half: f64, pub slippage: f64 }
pub fn entry_costs(cfg: &Config, ts: i64, atr: f64, units: f64, equity: f64) -> FillCosts {
    FillCosts { spread_half: effective_spread(cfg, ts, atr) / 2.0,
                slippage: effective_slippage(cfg, units, equity, atr) }
}
pub fn exit_costs(cfg: &Config, units: f64, equity: f64, atr: f64) -> FillCosts {
    FillCosts { spread_half: 0.0, slippage: effective_slippage(cfg, units, equity, atr) }
}