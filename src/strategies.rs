#![allow(dead_code)]

use crate::indicators::*;
use crate::predictor::{extract_features, CalibratedPredictor};
use crate::types::*;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub struct FlowState { pub delta_ratio: f64, pub vol_ratio: f64, pub stacked: i32 }

pub trait OrderFlowFeed: Send {
    fn update(&mut self, bar: &Bar) -> Option<FlowState>;
}

pub struct ProxyOrderFlow {
    min_delta: f64,
    min_vol: f64,
    vol: Sma,
    stacked: i32,
    last_dir: i32,
}

impl ProxyOrderFlow {
    pub fn new(cfg: &Config) -> Self {
        Self {
            min_delta: cfg.min_delta_ratio,
            min_vol: cfg.min_vol_ratio,
            vol: Sma::new(cfg.vol_sma_period),
            stacked: 0,
            last_dir: 0,
        }
    }
}

impl OrderFlowFeed for ProxyOrderFlow {
    fn update(&mut self, bar: &Bar) -> Option<FlowState> {
        let avg = self.vol.update(bar.volume)?;
        let dr = 2.0 * (bar.close - bar.low) / bar.range() - 1.0;
        let vr = if avg > 0.0 { bar.volume / avg } else { 1.0 };
        let strong = dr.abs() >= self.min_delta && vr >= self.min_vol;
        if strong {
            let dir = if dr > 0.0 { 1 } else { -1 };
            if dir == self.last_dir { self.stacked += dir; }
            else { self.stacked = dir; self.last_dir = dir; }
        } else { self.stacked = 0; self.last_dir = 0; }
        Some(FlowState { delta_ratio: dr, vol_ratio: vr, stacked: self.stacked })
    }
}

pub struct FootprintOrderFlow { _x: i32 }
impl OrderFlowFeed for FootprintOrderFlow {
    fn update(&mut self, _bar: &Bar) -> Option<FlowState> { None }
}

pub struct TfAggregator { bucket: i64, cur: Option<Bar> }
impl TfAggregator {
    pub fn new(bucket_secs: i64) -> Self { Self { bucket: bucket_secs.max(1), cur: None } }
    pub fn push(&mut self, bar: &Bar) -> Option<Bar> {
        let b = bar.ts - bar.ts.rem_euclid(self.bucket);
        match self.cur {
            None => {
                self.cur = Some(Bar { ts: b, open: bar.open, high: bar.high,
                    low: bar.low, close: bar.close, volume: bar.volume });
                None
            }
            Some(mut c) if c.ts == b => {
                c.high = c.high.max(bar.high);
                c.low = c.low.min(bar.low);
                c.close = bar.close;
                c.volume += bar.volume;
                self.cur = Some(c);
                None
            }
            Some(c) => {
                self.cur = Some(Bar { ts: b, open: bar.open, high: bar.high,
                    low: bar.low, close: bar.close, volume: bar.volume });
                Some(c)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Eval {
    pub bar_index: usize, pub atr: Option<f64>, pub adx: Option<f64>, pub rsi: Option<f64>,
    pub trend_up: bool, pub trend_ready: bool, pub squeeze_ok: bool,
    pub flow: Option<FlowState>, pub signals: Vec<Signal>, pub prediction: Option<f64>,
}

impl Eval {
    pub fn best(&self) -> Option<Signal> {
        self.signals.iter().copied().min_by_key(|s| s.kind.priority())
    }
}

pub struct StrategyEngine {
    pub bar_index: usize, cfg: Config, rsi: Rsi, adx: Adx, atr: Atr,
    ema_fast: Ema, ema_slow: Ema, trend_ema: Ema, tf5: TfAggregator,
    bb: Bollinger, kc: Keltner, donch: Donchian, mom: LinReg,
    prev_rsi: Option<f64>, squeeze_active: bool, bars_since_release: Option<usize>,
    flow: Box<dyn OrderFlowFeed>, recent_bars: VecDeque<Bar>,
    predictor: Option<CalibratedPredictor>, prediction_threshold: f64,
}

impl StrategyEngine {
    pub fn new(cfg: Config) -> Self {
        let flow: Box<dyn OrderFlowFeed> = Box::new(ProxyOrderFlow::new(&cfg));
        Self {
            bar_index: 0, rsi: Rsi::new(cfg.rsi_period), adx: Adx::new(cfg.adx_period),
            atr: Atr::new(cfg.atr_period), ema_fast: Ema::new(cfg.ema_fast),
            ema_slow: Ema::new(cfg.ema_slow), trend_ema: Ema::new(cfg.trend_ema_period),
            tf5: TfAggregator::new(300), bb: Bollinger::new(cfg.bb_period, cfg.bb_mult),
            kc: Keltner::new(cfg.kc_period, cfg.kc_mult),
            donch: Donchian::new(cfg.donchian_period), mom: LinReg::new(cfg.squeeze_mom_period),
            prev_rsi: None, squeeze_active: false, bars_since_release: None, flow,
            recent_bars: VecDeque::with_capacity(61),
            predictor: None, prediction_threshold: 0.60, cfg,
        }
    }

    pub fn with_predictor(cfg: Config, predictor: CalibratedPredictor) -> Self {
        let mut e = Self::new(cfg);
        e.prediction_threshold = predictor.config.probability_threshold;
        e.predictor = Some(predictor);
        e
    }

    pub fn on_bar(&mut self, bar: &Bar) -> Eval {
        self.bar_index += 1;
        if let Some(done) = self.tf5.push(bar) { self.trend_ema.update(done.close); }
        self.prev_rsi = self.rsi.value();
        let rsi_now = self.rsi.update(bar.close);
        let adx = self.adx.update(bar.high, bar.low, bar.close);
        let atr = self.atr.update(bar.high, bar.low, bar.close);
        let _ = self.ema_fast.update(bar.close);
        let _ = self.ema_slow.update(bar.close);
        let bb = self.bb.update(bar.close);
        let kc = self.kc.update(bar.high, bar.low, bar.close);
        let donch_prior = self.donch.prior();
        self.donch.update(bar.high, bar.low);
        let mom = match donch_prior {
            Some((hi, lo)) => self.mom.update(bar.close - 0.5 * (hi + lo)),
            None => None,
        };

        let in_sq = match (bb, kc) {
            (Some((bu, _, bl)), Some((ku, kl))) => bu < ku && bl > kl,
            _ => false,
        };
        if in_sq { self.squeeze_active = true; self.bars_since_release = None; }
        else if self.squeeze_active { self.squeeze_active = false; self.bars_since_release = Some(0); }
        else if let Some(n) = self.bars_since_release { self.bars_since_release = Some(n + 1); }
        let squeeze_ok = self.bars_since_release.map(|n| n <= self.cfg.squeeze_lookback).unwrap_or(false);

        let flow = self.flow.update(bar);
        let trend_ready = self.trend_ema.value().is_some();
        let trend_up = match self.trend_ema.value() { Some(e) => bar.close > e, None => false };
        let adx_ok = adx.map(|a| a >= self.cfg.adx_min).unwrap_or(false);

        let mut signals: Vec<Signal> = Vec::new();

        // RSI
        if trend_ready {
            if let (Some(pr), Some(rn)) = (self.prev_rsi, rsi_now) {
                if pr < self.cfg.rsi_low && rn >= self.cfg.rsi_low && trend_up && adx_ok {
                    signals.push(Signal { side: Side::Long, kind: StrategyKind::Rsi });
                }
                if pr > self.cfg.rsi_high && rn <= self.cfg.rsi_high && !trend_up && adx_ok {
                    signals.push(Signal { side: Side::Short, kind: StrategyKind::Rsi });
                }
            }
        }

        // Breakout
        if let Some((hi, lo)) = donch_prior {
            if adx_ok {
                if bar.close > hi { signals.push(Signal { side: Side::Long, kind: StrategyKind::Breakout }); }
                if bar.close < lo { signals.push(Signal { side: Side::Short, kind: StrategyKind::Breakout }); }
            }
        }

        // Squeeze
        if squeeze_ok {
            if let (Some(m), Some((bu, _, bl))) = (mom, bb) {
                if bar.close > bu && m > 0.0 { signals.push(Signal { side: Side::Long, kind: StrategyKind::Squeeze }); }
                if bar.close < bl && m < 0.0 { signals.push(Signal { side: Side::Short, kind: StrategyKind::Squeeze }); }
            }
        }

        // Order flow
        let mut flow_dir: i32 = 0;
        if let Some(f) = flow {
            if f.stacked.abs() >= self.cfg.required_stacked {
                flow_dir = f.stacked.signum();
                signals.push(Signal { side: if flow_dir > 0 { Side::Long } else { Side::Short }, kind: StrategyKind::OrderFlow });
            }
        }

        // Confluence gates
        let mut filtered: Vec<Signal> = Vec::new();
        for s in signals {
            if s.kind != StrategyKind::Squeeze && self.cfg.require_squeeze_gate && !squeeze_ok { continue; }
            if s.kind != StrategyKind::OrderFlow && self.cfg.require_orderflow_confirm {
                let need = match s.side { Side::Long => 1, Side::Short => -1 };
                if flow_dir != need { continue; }
            }
            filtered.push(s);
        }

        // Prediction
        let partial = Eval {
            bar_index: self.bar_index, atr, adx, rsi: rsi_now, trend_up, trend_ready,
            squeeze_ok, flow, signals: filtered.clone(), prediction: None,
        };
        let prediction = if let Some(pred) = &self.predictor {
            if self.recent_bars.len() >= 60 {
                let f = extract_features(bar, &partial, &self.recent_bars);
                Some(pred.predict_proba(&f))
            } else { None }
        } else { None };

        if let Some(p) = prediction {
            if p >= self.prediction_threshold {
                filtered.push(Signal { side: Side::Long, kind: StrategyKind::Prediction });
            } else if p <= 1.0 - self.prediction_threshold {
                filtered.push(Signal { side: Side::Short, kind: StrategyKind::Prediction });
            }
        }

        self.recent_bars.push_back(*bar);
        if self.recent_bars.len() > 60 { self.recent_bars.pop_front(); }

        Eval { bar_index: self.bar_index, atr, adx, rsi: rsi_now, trend_up, trend_ready,
               squeeze_ok, flow, signals: filtered, prediction }
    }
}