#![allow(dead_code)]

use crate::costs;
use crate::predictor::CalibratedPredictor;
use crate::strategies::{Eval, StrategyEngine};
use crate::types::*;

#[derive(Debug, Clone, Copy)]
pub struct PendingSignal { pub sig: Signal, pub atr_at_signal: f64, pub signal_bar_index: usize }

pub struct SessionState {
    pub engine: StrategyEngine, pub pos: Option<Position>, pub trades: Vec<Trade>,
    pub equity: f64, pub cfg: Config,
    pub last_eval: Option<Eval>, pub last_bar: Option<Bar>,
    pub pending_signal: Option<PendingSignal>, pub missed_trades: usize,
    pub realized_slippage_sum: f64, pub realized_slippage_n: usize,
    day_start_equity: f64, current_day: i64, trades_today: usize, halted: bool,
}

impl SessionState {
    pub fn new(cfg: Config) -> Self {
        let equity = cfg.initial_equity;
        Self {
            engine: StrategyEngine::new(cfg.clone()),
            pos: None, trades: Vec::new(), equity, cfg,
            last_eval: None, last_bar: None, pending_signal: None, missed_trades: 0,
            realized_slippage_sum: 0.0, realized_slippage_n: 0,
            day_start_equity: equity, current_day: 0, trades_today: 0, halted: false,
        }
    }
    pub fn with_predictor(cfg: Config, predictor: CalibratedPredictor) -> Self {
        let mut s = Self::new(cfg);
        s.engine = StrategyEngine::with_predictor(s.cfg.clone(), predictor);
        s
    }
    pub fn step(&mut self, bar: &Bar) {
        let day = bar.ts / 86_400;
        if day != self.current_day {
            self.current_day = day; self.day_start_equity = self.equity;
            self.trades_today = 0; self.halted = false;
        }
        if self.halted { return; }
        if let Some(p) = self.pending_signal.take() { self.try_fill_at_open(bar, p); }
        let eval = self.engine.on_bar(bar);
        self.last_eval = Some(eval.clone());
        self.last_bar = Some(*bar);
        if self.pos.is_some() { self.manage_position(bar, &eval); return; }
        if self.trades_today >= self.cfg.max_trades_per_day { return; }
        if self.equity < self.day_start_equity * (1.0 - self.cfg.daily_loss_limit) {
            self.halted = true; return;
        }
        for (s, e) in &self.cfg.news_blackouts {
            if bar.ts >= *s && bar.ts <= *e { return; }
        }
        if let Some(sig) = eval.best() {
            let atr = match eval.atr { Some(a) if a > 0.0 => a, _ => return };
            self.pending_signal = Some(PendingSignal { sig, atr_at_signal: atr, signal_bar_index: self.engine.bar_index });
        }
    }
    fn try_fill_at_open(&mut self, bar: &Bar, p: PendingSignal) {
        let atr = p.atr_at_signal; let open = bar.open;
        let risk_units = (self.equity * self.cfg.risk_per_trade) / (self.cfg.atr_sl_mult * atr);
        let max_units = (self.equity * self.cfg.max_leverage) / open;
        let units = risk_units.min(max_units);
        if units <= 0.0 { return; }
        let costs = costs::entry_costs(&self.cfg, bar.ts, atr, units, self.equity);
        let entry = match p.sig.side {
            Side::Long  => open + costs.spread_half + costs.slippage,
            Side::Short => open - costs.spread_half - costs.slippage,
        };
        let (sl, tp) = match p.sig.side {
            Side::Long  => (entry - self.cfg.atr_sl_mult * atr, entry + self.cfg.atr_tp_mult * atr),
            Side::Short => (entry + self.cfg.atr_sl_mult * atr, entry - self.cfg.atr_tp_mult * atr),
        };
        if let Some(prev) = self.last_bar {
            let gap = match p.sig.side {
                Side::Long  => (open - prev.close).max(0.0),
                Side::Short => (prev.close - open).max(0.0),
            };
            self.realized_slippage_sum += gap;
            self.realized_slippage_n += 1;
        }
        self.pos = Some(Position {
            side: p.sig.side, kind: p.sig.kind, entry, units, sl, tp,
            entry_ts: bar.ts, entry_bar: self.engine.bar_index,
            extreme: entry, be_moved: false, taken_partial: false,
        });
        self.trades_today += 1;
    }
    fn manage_position(&mut self, bar: &Bar, eval: &Eval) {
        let atr = eval.atr.unwrap_or(0.0);
        let time_stop_sl: Option<f64> = self.pos.as_ref().and_then(|p| {
            if bar.ts - p.entry_ts > self.cfg.max_seconds_in_trade { Some(p.sl) } else { None }
        });
        if let Some(sl) = time_stop_sl {
            self.close_position(bar.ts, sl, ExitReason::TimeStop);
            return;
        }
        let p = match self.pos.as_mut() { Some(p) => p, None => return };
        match p.side {
            Side::Long  => p.extreme = p.extreme.max(bar.high),
            Side::Short => p.extreme = p.extreme.min(bar.low),
        }
        let favorable = match p.side { Side::Long => p.extreme - p.entry, Side::Short => p.entry - p.extreme };
        if !p.be_moved && atr > 0.0 && favorable >= self.cfg.trail_after_atr * atr {
            p.sl = p.entry; p.be_moved = true;
        }
        if p.be_moved && atr > 0.0 {
            let trail = match p.side {
                Side::Long  => p.extreme - self.cfg.trail_atr_mult * atr,
                Side::Short => p.extreme + self.cfg.trail_atr_mult * atr,
            };
            match p.side {
                Side::Long  if trail > p.sl => p.sl = trail,
                Side::Short if trail < p.sl => p.sl = trail,
                _ => {}
            }
        }
        if !p.taken_partial && atr > 0.0 && favorable >= self.cfg.partial_atr_mult * atr {
            let exit_price = match p.side {
                Side::Long  => p.entry + self.cfg.partial_atr_mult * atr,
                Side::Short => p.entry - self.cfg.partial_atr_mult * atr,
            };
            let half_units = p.units * 0.5;
            let costs = costs::exit_costs(&self.cfg, half_units, self.equity, atr);
            let exit = exit_price + match p.side { Side::Long => -costs.slippage, Side::Short => costs.slippage };
            let pnl = half_units * (exit - p.entry) * p.side.dir() - self.cfg.commission_per_unit * half_units * 2.0;
            self.equity += pnl;
            self.trades.push(Trade {
                side: p.side, kind: p.kind, entry_ts: p.entry_ts, exit_ts: bar.ts,
                entry: p.entry, exit, units: half_units, pnl,
                reason: ExitReason::PartialTakeProfit,
                bars_held: self.engine.bar_index - p.entry_bar,
            });
            p.units -= half_units; p.taken_partial = true;
            p.sl = p.entry; p.be_moved = true;
        }
        let p = self.pos.as_ref().unwrap();
        let hit_sl = match p.side { Side::Long => bar.low <= p.sl, Side::Short => bar.high >= p.sl };
        let hit_tp = match p.side { Side::Long => bar.high >= p.tp, Side::Short => bar.low <= p.tp };
        if !hit_sl && !hit_tp { return; }
        let (exit, reason) = if hit_sl && hit_tp {
            let dist_sl = (bar.open - p.sl).abs(); let dist_tp = (bar.open - p.tp).abs();
            if dist_sl < dist_tp { (p.sl, ExitReason::StopLoss) } else { (p.tp, ExitReason::TakeProfit) }
        } else if hit_sl { (p.sl, ExitReason::StopLoss) } else { (p.tp, ExitReason::TakeProfit) };
        self.close_position(bar.ts, exit, reason);
    }
    fn close_position(&mut self, ts: i64, exit_raw: f64, reason: ExitReason) {
        let atr = self.last_eval.as_ref().and_then(|e| e.atr).unwrap_or(0.0);
        if let Some(p) = self.pos.take() {
            let costs = costs::exit_costs(&self.cfg, p.units, self.equity, atr);
            let exit = exit_raw + match p.side { Side::Long => -costs.slippage, Side::Short => costs.slippage };
            let pnl = p.units * (exit - p.entry) * p.side.dir() - self.cfg.commission_per_unit * p.units * 2.0;
            self.equity += pnl;
            self.trades.push(Trade {
                side: p.side, kind: p.kind, entry_ts: p.entry_ts, exit_ts: ts,
                entry: p.entry, exit, units: p.units, pnl, reason,
                bars_held: self.engine.bar_index - p.entry_bar,
            });
        }
    }
    pub fn force_close(&mut self, last_bar: &Bar) {
        if self.pos.is_some() { self.close_position(last_bar.ts, last_bar.close, ExitReason::EndOfData); }
    }
    pub fn halted(&self) -> bool { self.halted }
}