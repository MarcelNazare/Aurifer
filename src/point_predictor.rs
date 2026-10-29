#![allow(dead_code)]

use crate::predictor::{extract_features, N_FEATURES};
use crate::strategies::Eval;
use crate::types::Bar;
use anyhow::Result;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct PointPredictorConfig {
    pub horizon: usize, pub l2: f64, pub epochs: usize,
    pub learning_rate: f64, pub train_fraction: f64, pub min_train_bars: usize,
}
impl Default for PointPredictorConfig {
    fn default() -> Self {
        Self { horizon: 30, l2: 0.001, epochs: 2000, learning_rate: 0.01,
               train_fraction: 0.70, min_train_bars: 500 }
    }
}

#[derive(Debug, Clone)]
pub struct RidgeModel { pub weights: Vec<f64>, pub bias: f64 }
impl RidgeModel {
    pub fn zeros(n: usize) -> Self { Self { weights: vec![0.0; n], bias: 0.0 } }
    pub fn train(x: &[Vec<f64>], y: &[f64], epochs: usize, lr: f64, l2: f64) -> Self {
        let n = x.len(); if n == 0 { return Self::zeros(0); }
        let nf = x[0].len(); let mut m = Self::zeros(nf);
        for _ in 0..epochs {
            let mut gw = vec![0.0; nf]; let mut gb = 0.0;
            for (xi, &yi) in x.iter().zip(y.iter()) {
                let pred: f64 = m.weights.iter().zip(xi.iter()).map(|(w, v)| w * v).sum::<f64>() + m.bias;
                let err = pred - yi;
                for (g, v) in gw.iter_mut().zip(xi.iter()) { *g += err * v; }
                gb += err;
            }
            let inv = 1.0 / n as f64;
            for (w, g) in m.weights.iter_mut().zip(gw.iter()) { *w -= lr * (g * inv + l2 * *w); }
            m.bias -= lr * gb * inv;
        }
        m
    }
    pub fn predict(&self, x: &[f64]) -> f64 {
        self.weights.iter().zip(x.iter()).map(|(w, v)| w * v).sum::<f64>() + self.bias
    }
}

#[derive(Debug, Clone)]
pub struct PointMetrics {
    pub mae: f64, pub median_error: f64, pub rmse: f64,
    pub pct_within_half_spread: f64, pub pct_within_spread: f64, pub pct_within_sl: f64,
    pub baseline_mae: f64, pub improvement: f64,
    pub n_test: usize, pub n_actionable: usize, pub n_actionable_profitable: usize,
    pub actionable_pnl: f64, pub actionable_accuracy: f64,
}

pub struct PointPredictor {
    pub model: RidgeModel, pub feature_mean: Vec<f64>, pub feature_std: Vec<f64>,
    pub target_mean: f64, pub target_std: f64,
    pub config: PointPredictorConfig, pub metrics: PointMetrics,
    pub samples: Vec<(f64, f64, f64)>,
}

impl PointPredictor {
    pub fn train(bars: &[Bar], evals: &[Eval], cfg: PointPredictorConfig,
                 spread: f64, slippage: f64, typical_sl: f64) -> Result<Self> {
        anyhow::ensure!(bars.len() == evals.len(), "alignment");
        let n = bars.len();
        anyhow::ensure!(n >= cfg.min_train_bars, "need >= {} bars", cfg.min_train_bars);
        let mut features: Vec<Option<[f64; N_FEATURES]>> = Vec::with_capacity(n);
        let mut targets: Vec<Option<f64>> = Vec::with_capacity(n);
        let mut prices: Vec<f64> = Vec::with_capacity(n);
        let mut recent: VecDeque<Bar> = VecDeque::with_capacity(61);
        for i in 0..n {
            let f = if evals[i].atr.is_some() { Some(extract_features(&bars[i], &evals[i], &recent)) } else { None };
            let t = if i + cfg.horizon < n { Some(bars[i + cfg.horizon].close - bars[i].close) } else { None };
            features.push(f); targets.push(t); prices.push(bars[i].close);
            recent.push_back(bars[i]); if recent.len() > 60 { recent.pop_front(); }
        }
        let split = ((n as f64) * cfg.train_fraction) as usize;
        anyhow::ensure!(split + cfg.horizon < n, "train split too late");
        let tr_idx: Vec<usize> = (0..split).filter(|&i| features[i].is_some() && targets[i].is_some()).collect();
        let te_idx: Vec<usize> = (split..n).filter(|&i| features[i].is_some() && targets[i].is_some()).collect();
        anyhow::ensure!(tr_idx.len() >= 200, "not enough training samples");
        anyhow::ensure!(!te_idx.is_empty(), "no test samples");
        let mut fmean = vec![0.0; N_FEATURES];
        for &i in &tr_idx { if let Some(f) = &features[i] { for (m, v) in fmean.iter_mut().zip(f.iter()) { *m += v; } } }
        for m in fmean.iter_mut() { *m /= tr_idx.len() as f64; }
        let mut fvar = vec![0.0; N_FEATURES];
        for &i in &tr_idx {
            if let Some(f) = &features[i] {
                for (s, (v, m)) in fvar.iter_mut().zip(f.iter().zip(fmean.iter())) { *s += (v - m) * (v - m); }
            }
        }
        let fstd: Vec<f64> = fvar.iter().map(|v| (v / tr_idx.len() as f64).sqrt().max(1e-6)).collect();
        let tsum: f64 = tr_idx.iter().filter_map(|&i| targets[i]).sum();
        let tmean = tsum / tr_idx.len() as f64;
        let tvar: f64 = tr_idx.iter().filter_map(|&i| targets[i].map(|t| (t - tmean).powi(2))).sum::<f64>() / tr_idx.len() as f64;
        let tstd = tvar.sqrt().max(1e-6);
        let x_tr: Vec<Vec<f64>> = tr_idx.iter().map(|&i| features[i].unwrap().iter()
            .zip(fmean.iter().zip(fstd.iter())).map(|(v, (m, s))| (v - m) / s).collect()).collect();
        let y_tr: Vec<f64> = tr_idx.iter().map(|&i| (targets[i].unwrap() - tmean) / tstd).collect();
        let model = RidgeModel::train(&x_tr, &y_tr, cfg.epochs, cfg.learning_rate, cfg.l2);
        let mut errors = Vec::with_capacity(te_idx.len());
        let mut baseline_errors = Vec::with_capacity(te_idx.len());
        let mut samples: Vec<(f64, f64, f64)> = Vec::with_capacity(10);
        let mut actionable = 0usize; let mut actionable_profitable = 0usize;
        let mut actionable_pnl = 0.0; let cost = spread + slippage;
        for &i in &te_idx {
            let x: Vec<f64> = features[i].unwrap().iter().zip(fmean.iter().zip(fstd.iter()))
                .map(|(v, (m, s))| (v - m) / s).collect();
            let pred_delta = model.predict(&x) * tstd + tmean;
            let actual_delta = targets[i].unwrap();
            let current = prices[i]; let predicted = current + pred_delta; let actual = current + actual_delta;
            errors.push((pred_delta - actual_delta).abs());
            baseline_errors.push(actual_delta.abs());
            if pred_delta.abs() > cost {
                actionable += 1;
                let pnl = if pred_delta > 0.0 { actual_delta - cost } else { -actual_delta - cost };
                actionable_pnl += pnl;
                if pnl > 0.0 { actionable_profitable += 1; }
            }
            if samples.len() < 10 { samples.push((current, predicted, actual)); }
        }
        errors.sort_by(|a, b| a.partial_cmp(b).unwrap());
        baseline_errors.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mae = errors.iter().sum::<f64>() / errors.len() as f64;
        let median_error = errors[errors.len() / 2];
        let rmse = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len() as f64).sqrt();
        let baseline_mae = baseline_errors.iter().sum::<f64>() / baseline_errors.len() as f64;
        let improvement = if baseline_mae > 0.0 { 1.0 - mae / baseline_mae } else { 0.0 };
        let pct_within_half = errors.iter().filter(|&&e| e <= spread / 2.0).count() as f64 / errors.len() as f64;
        let pct_within_spread = errors.iter().filter(|&&e| e <= spread).count() as f64 / errors.len() as f64;
        let pct_within_sl = errors.iter().filter(|&&e| e <= typical_sl).count() as f64 / errors.len() as f64;
        let actionable_accuracy = if actionable > 0 { actionable_profitable as f64 / actionable as f64 } else { 0.0 };
        Ok(Self { model, feature_mean: fmean, feature_std: fstd, target_mean: tmean, target_std: tstd,
                  config: cfg,
                  metrics: PointMetrics { mae, median_error, rmse,
                      pct_within_half_spread: pct_within_half, pct_within_spread, pct_within_sl,
                      baseline_mae, improvement, n_test: te_idx.len(),
                      n_actionable: actionable, n_actionable_profitable: actionable_profitable,
                      actionable_pnl, actionable_accuracy }, samples })
    }
}

pub fn print_diagnostic(p: &PointPredictor, spread: f64, slippage: f64, typical_sl: f64) {
    let m = &p.metrics; let cost = spread + slippage;
    println!();
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  POINT PREDICTION DIAGNOSTIC");
    println!("═══════════════════════════════════════════════════════════════════");
    println!("  Goal:  predict exact price at t+{} bars", p.config.horizon);
    println!("  Model: ridge regression on {} features", N_FEATURES);
    println!("  Test:  {} samples", m.n_test);
    println!();
    println!("  ─── Error distribution ───");
    println!("  Mean absolute error:   ${:.4}", m.mae);
    println!("  Median absolute error: ${:.4}", m.median_error);
    println!("  RMSE:                  ${:.4}", m.rmse);
    println!();
    println!("  ─── vs random-walk baseline ───");
    println!("  Baseline MAE (predict no change): ${:.4}", m.baseline_mae);
    println!("  Improvement over baseline:        {:.1}%", m.improvement * 100.0);
    if m.improvement < 0.10 { println!("  ⚠️  Improvement < 10% — model adds essentially no predictive value."); }
    println!();
    println!("  ─── % of predictions within tolerance ───");
    println!("  Within half-spread (${:.2}):  {:5.1}%", spread / 2.0, m.pct_within_half_spread * 100.0);
    println!("  Within spread      (${:.2}):  {:5.1}%", spread, m.pct_within_spread * 100.0);
    println!("  Within SL          (${:.2}):  {:5.1}%", typical_sl, m.pct_within_sl * 100.0);
    println!();
    println!("  ─── If you traded every prediction above cost (${:.2} round-trip) ───", cost);
    println!("  Actionable predictions:  {} of {} ({:.1}%)", m.n_actionable, m.n_test,
             m.n_actionable as f64 / m.n_test.max(1) as f64 * 100.0);
    println!("  Profitable actionable:   {} ({:.1}% win rate)", m.n_actionable_profitable, m.actionable_accuracy * 100.0);
    println!("  Estimated total PnL:     ${:+.2}", m.actionable_pnl);
    println!();
    println!("  ─── Sample predictions (first 10 test bars) ───");
    println!("  {:>10}  {:>10}  {:>10}  {:>10}", "Current", "Predicted", "Actual", "Error");
    for (cur, pred, act) in &p.samples {
        println!("  {:>10.2}  {:>10.2}  {:>10.2}  {:>10.2}", cur, pred, act, (pred - act).abs());
    }
    println!();
    let tradeable = m.pct_within_half_spread > 0.90 && m.actionable_pnl > 0.0;
    if tradeable {
        println!("  ✅ TRADEABLE");
        println!("     Point prediction is within half-spread >90% of the time.");
    } else {
        println!("  ❌ NOT TRADEABLE");
        println!();
        println!("     To profit from point prediction, >90% of predictions must be");
        println!("     accurate within half-spread. Your model achieves: {:.1}%.", m.pct_within_half_spread * 100.0);
        println!();
        println!("     Average error is ${:.2} = {:.1}× the half-spread.", m.mae, m.mae / (spread / 2.0).max(0.01));
        println!("     The model is off by more than your costs on {:.1}% of predictions.",
                 (1.0 - m.pct_within_spread) * 100.0);
        if m.n_actionable > 0 {
            println!();
            println!("     Trading every actionable prediction would have produced: ${:+.2}", m.actionable_pnl);
            println!("     Win rate on actionable trades: {:.1}% (needs >50% after costs)", m.actionable_accuracy * 100.0);
        }
        println!();
        println!("     This is not a bug in the model — it's physics. At 1-minute resolution,");
        println!("     gold's noise is larger than any predictable signal.");
    }
    println!("═══════════════════════════════════════════════════════════════════");
}