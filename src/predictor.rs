#![allow(dead_code)]

use crate::strategies::Eval;
use crate::types::Bar;
use anyhow::Result;
use std::collections::{HashMap, VecDeque};

pub const N_FEATURES: usize = 15;

#[derive(Debug, Clone)]
pub struct PredictorConfig {
    pub horizon: usize, pub label_atr_mult: f64,
    pub epochs: usize, pub learning_rate: f64, pub l2: f64,
    pub train_fraction: f64, pub probability_threshold: f64, pub min_train_bars: usize,
}

impl Default for PredictorConfig {
    fn default() -> Self {
        Self { horizon: 30, label_atr_mult: 0.30, epochs: 500, learning_rate: 0.05, l2: 0.001,
               train_fraction: 0.70, probability_threshold: 0.60, min_train_bars: 500 }
    }
}

pub fn extract_features(bar: &Bar, eval: &Eval, recent: &VecDeque<Bar>) -> [f64; N_FEATURES] {
    let atr = eval.atr.unwrap_or(0.0).max(1e-9);
    let rsi = eval.rsi.unwrap_or(50.0);
    let adx = eval.adx.unwrap_or(0.0);
    let range = (bar.high - bar.low).max(1e-9);
    let close_loc = (bar.close - bar.low) / range;
    let body_ratio = (bar.close - bar.open).abs() / range;
    let atr_ret = |n: usize| -> f64 {
        if recent.len() < n { return 0.0; }
        let then = recent[recent.len() - n].close;
        if then.abs() < 1e-9 { 0.0 } else { ((bar.close - then) / atr).clamp(-40.0, 40.0) }
    };
    let ret_5 = atr_ret(5); let ret_20 = atr_ret(20); let ret_60 = atr_ret(60);
    let ema20 = sma_of(recent, 20); let ema50 = sma_of(recent, 50);
    let d_ema20 = ((bar.close - ema20) / atr).clamp(-10.0, 10.0);
    let d_ema50 = ((bar.close - ema50) / atr).clamp(-10.0, 10.0);
    let mut streak: i32 = 0;
    for w in recent.iter().rev().take(10) {
        if w.close > w.open { if streak < 0 { break; } streak += 1; }
        else if w.close < w.open { if streak > 0 { break; } streak -= 1; }
        else { break; }
    }
    let streak_f = (streak as f64).clamp(-10.0, 10.0) / 10.0;
    let atr_avg = atr_avg_of(recent, 20).max(1e-9);
    let vol_ratio = ((atr / atr_avg) / 5.0).clamp(0.02, 2.0);
    let rsi_dist = (rsi - 50.0) / 50.0;
    let squeeze = if eval.squeeze_ok { 1.0 } else { 0.0 };
    let trend_up = if eval.trend_up { 1.0 } else { -1.0 };
    [rsi / 100.0, adx / 50.0, vol_ratio, close_loc, body_ratio,
     d_ema20, d_ema50, ret_5 / 10.0, ret_20 / 20.0, ret_60 / 40.0,
     streak_f, squeeze, rsi_dist, trend_up, bar.volume / 1000.0]
}

fn sma_of(recent: &VecDeque<Bar>, period: usize) -> f64 {
    if recent.len() < period || period == 0 { return 0.0; }
    let sum: f64 = recent.iter().rev().take(period).map(|b| b.close).sum();
    sum / period as f64
}

fn atr_avg_of(recent: &VecDeque<Bar>, period: usize) -> f64 {
    if recent.len() < period + 1 || period == 0 { return 1.0; }
    let mut sum = 0.0;
    let mut prev = recent[recent.len() - period - 1].close;
    for b in recent.iter().skip(recent.len() - period) {
        let tr = (b.high - b.low).max((b.high - prev).abs()).max((b.low - prev).abs());
        sum += tr; prev = b.close;
    }
    sum / period as f64
}

pub fn make_label(bars: &[Bar], t: usize, horizon: usize, mult: f64, atr_t: f64) -> Option<bool> {
    if t + horizon >= bars.len() || atr_t <= 0.0 { return None; }
    Some(bars[t + horizon].close - bars[t].close > mult * atr_t)
}

fn sigmoid(z: f64) -> f64 { if z >= 0.0 { 1.0 / (1.0 + (-z).exp()) } else { let e = z.exp(); e / (1.0 + e) } }
fn dot(a: &[f64], b: &[f64]) -> f64 { a.iter().zip(b.iter()).map(|(x, y)| x * y).sum() }

#[derive(Debug, Clone)]
pub struct LogisticModel { pub weights: Vec<f64>, pub bias: f64 }

impl LogisticModel {
    pub fn zeros(n: usize) -> Self { Self { weights: vec![0.0; n], bias: 0.0 } }
    pub fn train(x: &[Vec<f64>], y: &[bool], epochs: usize, lr: f64, l2: f64) -> Self {
        let n = x.len(); if n == 0 { return Self::zeros(0); }
        let nf = x[0].len(); let mut m = Self::zeros(nf);
        for _ in 0..epochs {
            let mut gw = vec![0.0; nf]; let mut gb = 0.0;
            for (xi, &yi) in x.iter().zip(y.iter()) {
                let p = sigmoid(dot(&m.weights, xi) + m.bias);
                let err = p - if yi { 1.0 } else { 0.0 };
                for (g, v) in gw.iter_mut().zip(xi.iter()) { *g += err * v; }
                gb += err;
            }
            let inv = 1.0 / n as f64;
            for (w, g) in m.weights.iter_mut().zip(gw.iter()) { *w -= lr * (g * inv + l2 * *w); }
            m.bias -= lr * gb * inv;
        }
        m
    }
    pub fn predict_proba(&self, x: &[f64]) -> f64 { sigmoid(dot(&self.weights, x) + self.bias) }
}

#[derive(Debug, Clone)]
pub struct EvalMetrics {
    pub n_train: usize, pub n_test: usize, pub base_rate: f64, pub accuracy: f64,
    pub auc: f64, pub brier: f64, pub log_loss: f64,
}

#[derive(Debug, Clone)]
pub struct CalibratedPredictor {
    pub raw: LogisticModel, pub platt_a: f64, pub platt_b: f64,
    pub feature_mean: Vec<f64>, pub feature_std: Vec<f64>,
    pub config: PredictorConfig, pub metrics: EvalMetrics,
    pub auc_ci_95: (f64, f64), pub brier_calibrated: f64,
}

impl CalibratedPredictor {
    pub fn train_calibrated(bars: &[Bar], evals: &[Eval], cfg: PredictorConfig) -> Result<Self> {
        anyhow::ensure!(bars.len() == evals.len(), "alignment");
        let (features, labels, atrs) = build_dataset(bars, evals, &cfg)?;
        let n = bars.len();
        let usable: Vec<usize> = (0..n).filter(|&i| features[i].is_some() && labels[i].is_some() && atrs[i] > 0.0).collect();
        let m = usable.len();
        anyhow::ensure!(m >= 200, "only {} usable samples", m);
        let i_tr = (m as f64 * 0.60) as usize;
        let i_cal = (m as f64 * 0.75) as usize;
        let tr_idx: Vec<usize> = usable[..i_tr].to_vec();
        let cal_idx: Vec<usize> = usable[i_tr..i_cal].to_vec();
        let test_idx: Vec<usize> = usable[i_cal..].to_vec();
        anyhow::ensure!(!tr_idx.is_empty() && !cal_idx.is_empty() && !test_idx.is_empty(), "empty split");
        let (mean, std) = fit_scaler(&features, &tr_idx);
        let norm = |f: &[f64; N_FEATURES]| -> Vec<f64> {
            f.iter().zip(mean.iter().zip(std.iter())).map(|(v, (m, s))| (v - m) / s).collect()
        };
        let x_tr: Vec<Vec<f64>> = tr_idx.iter().map(|&i| norm(features[i].as_ref().unwrap())).collect();
        let y_tr: Vec<bool>    = tr_idx.iter().map(|&i| labels[i].unwrap()).collect();
        let raw = LogisticModel::train(&x_tr, &y_tr, cfg.epochs, cfg.learning_rate, cfg.l2);
        let x_cal: Vec<Vec<f64>> = cal_idx.iter().map(|&i| norm(features[i].as_ref().unwrap())).collect();
        let y_cal: Vec<bool>    = cal_idx.iter().map(|&i| labels[i].unwrap()).collect();
        let (a, b) = fit_platt(&raw, &x_cal, &y_cal);
        let mut probs = Vec::new(); let mut ys = Vec::new();
        for &i in &test_idx {
            let x = norm(features[i].as_ref().unwrap());
            let logit = dot(&raw.weights, &x) + raw.bias;
            probs.push(sigmoid(a * logit + b));
            ys.push(labels[i].unwrap());
        }
        let metrics = evaluate(&probs, &ys, tr_idx.len());
        let brier_cal = brier(&probs, &ys);
        let ci = bootstrap_auc_ci(&probs, &ys, 500);
        println!("📊 Predictor trained (Platt-calibrated):");
        println!("   train / cal / test: {} / {} / {}", tr_idx.len(), cal_idx.len(), test_idx.len());
        println!("   base rate:          {:.1}%", metrics.base_rate * 100.0);
        println!("   accuracy:           {:.1}%", metrics.accuracy * 100.0);
        println!("   AUC:                {:.3} (95% CI {:.3}–{:.3})", metrics.auc, ci.0, ci.1);
        println!("   Brier (calibrated): {:.4}", brier_cal);
        println!("   Platt a,b:          {:.3}, {:.3}", a, b);
        if brier_cal > 0.24 { eprintln!("⚠️ Brier > 0.24 — poor calibration"); }
        if ci.0 < 0.50 { eprintln!("⚠️ AUC 95% CI includes 0.50 — not statistically significant"); }
        if a < 0.0 { eprintln!("⚠️ Platt a < 0 — raw model anti-correlated"); }
        Ok(Self { raw, platt_a: a, platt_b: b, feature_mean: mean, feature_std: std,
                  config: cfg, metrics, auc_ci_95: ci, brier_calibrated: brier_cal })
    }
    pub fn predict_proba(&self, features: &[f64; N_FEATURES]) -> f64 {
        let x: Vec<f64> = features.iter()
            .zip(self.feature_mean.iter().zip(self.feature_std.iter()))
            .map(|(v, (m, s))| (v - m) / s).collect();
        let logit = dot(&self.raw.weights, &x) + self.raw.bias;
        sigmoid(self.platt_a * logit + self.platt_b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime { TrendingUp, TrendingDown, Ranging, Transition }

impl Regime {
    pub fn classify(adx: f64, trend_up: bool) -> Self {
        if adx >= 25.0 { if trend_up { Regime::TrendingUp } else { Regime::TrendingDown } }
        else if adx < 20.0 { Regime::Ranging }
        else { Regime::Transition }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Regime::TrendingUp => "TREND_UP", Regime::TrendingDown => "TREND_DN",
            Regime::Ranging => "RANGING", Regime::Transition => "TRANSITION",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FoldMetrics {
    pub fold: usize, pub train_bars: usize, pub test_bars: usize,
    pub auc: f64, pub accuracy: f64, pub base_rate: f64,
    pub per_regime: HashMap<&'static str, (usize, f64, f64)>,
}

pub struct WalkForwardResult {
    pub folds: Vec<FoldMetrics>, pub aggregate_auc: f64, pub aggregate_accuracy: f64,
    pub worst_fold_auc: f64, pub stddev_auc: f64,
    pub regime_summary: HashMap<&'static str, (usize, f64, f64)>,
}

impl WalkForwardResult {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.folds.len() >= 3, "need >=3 folds");
        anyhow::ensure!(self.aggregate_auc >= 0.54, "aggregate AUC {:.3} < 0.54", self.aggregate_auc);
        anyhow::ensure!(self.worst_fold_auc >= 0.50, "worst fold AUC {:.3} < 0.50", self.worst_fold_auc);
        let strong = self.regime_summary.values().filter(|(n, _a, auc)| *n >= 100 && *auc > 0.52).count();
        anyhow::ensure!(strong >= 2, "only {} strong regimes (need >=2)", strong);
        Ok(())
    }
}

pub fn walk_forward_validate(bars: &[Bar], evals: &[Eval], cfg: &PredictorConfig, n_folds: usize) -> Result<WalkForwardResult> {
    let (features, labels, atrs) = build_dataset(bars, evals, cfg)?;
    let n = bars.len();
    let fold_size = n / (n_folds + 1);
    anyhow::ensure!(fold_size > cfg.min_train_bars / 2, "folds too small");
    let mut folds = Vec::new();
    let mut all_regime: HashMap<&'static str, (usize, f64, f64)> = HashMap::new();
    for k in 1..=n_folds {
        let tr_end = fold_size * (k + 1);
        let te_end = if k == n_folds { n } else { fold_size * (k + 2) };
        let tr: Vec<usize> = (0..tr_end).filter(|&i| features[i].is_some() && labels[i].is_some() && atrs[i] > 0.0).collect();
        let te: Vec<usize> = (tr_end..te_end).filter(|&i| features[i].is_some() && labels[i].is_some() && atrs[i] > 0.0).collect();
        if tr.len() < 200 || te.len() < 50 { continue; }
        let (mean, std) = fit_scaler(&features, &tr);
        let norm = |f: &[f64; N_FEATURES]| -> Vec<f64> {
            f.iter().zip(mean.iter().zip(std.iter())).map(|(v, (m, s))| (v - m) / s).collect()
        };
        let xt: Vec<Vec<f64>> = tr.iter().map(|&i| norm(features[i].as_ref().unwrap())).collect();
        let yt: Vec<bool>    = tr.iter().map(|&i| labels[i].unwrap()).collect();
        let model = LogisticModel::train(&xt, &yt, cfg.epochs, cfg.learning_rate, cfg.l2);
        let mut probs = Vec::new(); let mut ys = Vec::new(); let mut regs = Vec::new();
        for &i in &te {
            let x = norm(features[i].as_ref().unwrap());
            probs.push(model.predict_proba(&x));
            ys.push(labels[i].unwrap());
            regs.push(Regime::classify(evals[i].adx.unwrap_or(0.0), evals[i].trend_up));
        }
        let met = evaluate(&probs, &ys, tr.len());
        let mut per_reg: HashMap<&'static str, (usize, f64, f64)> = HashMap::new();
        for r in [Regime::TrendingUp, Regime::TrendingDown, Regime::Ranging, Regime::Transition] {
            let idxs: Vec<usize> = (0..probs.len()).filter(|&i| regs[i] == r).collect();
            if idxs.is_empty() { continue; }
            let rp: Vec<f64> = idxs.iter().map(|&i| probs[i]).collect();
            let ry: Vec<bool> = idxs.iter().map(|&i| ys[i]).collect();
            let ra = auc_from_sorted(&rp, &ry);
            let acc = rp.iter().zip(ry.iter()).filter(|(p, &l)| (**p > 0.5) == l).count() as f64 / idxs.len() as f64;
            per_reg.insert(r.as_str(), (idxs.len(), acc, ra));
            let e = all_regime.entry(r.as_str()).or_insert((0, 0.0, 0.0));
            e.0 += idxs.len(); e.1 += acc * idxs.len() as f64; e.2 += ra * idxs.len() as f64;
        }
        folds.push(FoldMetrics { fold: k, train_bars: tr.len(), test_bars: te.len(),
            auc: met.auc, accuracy: met.accuracy, base_rate: met.base_rate, per_regime: per_reg });
    }
    anyhow::ensure!(!folds.is_empty(), "no valid folds");
    for (n, a_s, auc_s) in all_regime.values_mut() {
        if *n > 0 { *a_s /= *n as f64; *auc_s /= *n as f64; }
    }
    let aucs: Vec<f64> = folds.iter().map(|f| f.auc).collect();
    let agg = aucs.iter().sum::<f64>() / aucs.len() as f64;
    let worst = aucs.iter().cloned().fold(f64::INFINITY, f64::min);
    let var = aucs.iter().map(|a| (a - agg) * (a - agg)).sum::<f64>() / aucs.len() as f64;
    let stdv = var.sqrt();
    let acc = folds.iter().map(|f| f.accuracy).sum::<f64>() / folds.len() as f64;
    println!("📊 Walk-forward validation ({} folds):", folds.len());
    for f in &folds { println!("   fold {}: AUC {:.3}, acc {:.1}%, n={}", f.fold, f.auc, f.accuracy * 100.0, f.test_bars); }
    println!("   aggregate AUC:  {:.3} ± {:.3}", agg, stdv);
    println!("   worst fold AUC: {:.3}", worst);
    println!("   regime breakdown:");
    for (name, (n, a, auc)) in &all_regime { println!("      {:<12} n={:<6} acc={:.1}% auc={:.3}", name, n, a * 100.0, auc); }
    Ok(WalkForwardResult { folds, aggregate_auc: agg, aggregate_accuracy: acc,
                           worst_fold_auc: worst, stddev_auc: stdv, regime_summary: all_regime })
}

fn build_dataset(bars: &[Bar], evals: &[Eval], cfg: &PredictorConfig)
    -> Result<(Vec<Option<[f64; N_FEATURES]>>, Vec<Option<bool>>, Vec<f64>)>
{
    let n = bars.len();
    let mut feat = Vec::with_capacity(n); let mut lab = Vec::with_capacity(n); let mut atrs = Vec::with_capacity(n);
    let mut recent: VecDeque<Bar> = VecDeque::with_capacity(61);
    for i in 0..n {
        let bar = &bars[i]; let eval = &evals[i]; let atr = eval.atr.unwrap_or(0.0);
        let f = if eval.atr.is_some() { Some(extract_features(bar, eval, &recent)) } else { None };
        let l = if atr > 0.0 { make_label(bars, i, cfg.horizon, cfg.label_atr_mult, atr) } else { None };
        feat.push(f); lab.push(l); atrs.push(atr);
        recent.push_back(*bar);
        if recent.len() > 60 { recent.pop_front(); }
    }
    Ok((feat, lab, atrs))
}

fn fit_scaler(features: &[Option<[f64; N_FEATURES]>], idx: &[usize]) -> (Vec<f64>, Vec<f64>) {
    let mut mean = vec![0.0; N_FEATURES]; let mut cnt = 0usize;
    for &i in idx { if let Some(f) = &features[i] { for (m, v) in mean.iter_mut().zip(f.iter()) { *m += v; } cnt += 1; } }
    if cnt > 0 { for m in mean.iter_mut() { *m /= cnt as f64; } }
    let mut var = vec![0.0; N_FEATURES];
    for &i in idx {
        if let Some(f) = &features[i] {
            for (s, (v, m)) in var.iter_mut().zip(f.iter().zip(mean.iter())) { *s += (v - m) * (v - m); }
        }
    }
    let std: Vec<f64> = var.iter().map(|v| (v / cnt.max(1) as f64).sqrt().max(1e-6)).collect();
    (mean, std)
}

fn fit_platt(model: &LogisticModel, x: &[Vec<f64>], y: &[bool]) -> (f64, f64) {
    let mut a = 1.0; let mut b = 0.0; let lr = 0.05; let l2 = 1e-3;
    for _ in 0..200 {
        let mut ga = 0.0; let mut gb = 0.0;
        for (xi, &yi) in x.iter().zip(y.iter()) {
            let logit = dot(&model.weights, xi) + model.bias;
            let p = sigmoid(a * logit + b);
            let err = p - if yi { 1.0 } else { 0.0 };
            ga += err * logit; gb += err;
        }
        let inv = 1.0 / x.len() as f64;
        a -= lr * (ga * inv + l2 * a);
        b -= lr * gb * inv;
    }
    (a, b)
}

fn brier(p: &[f64], y: &[bool]) -> f64 {
    p.iter().zip(y.iter()).map(|(pi, &yi)| { let a = if yi { 1.0 } else { 0.0 }; (pi - a) * (pi - a) })
     .sum::<f64>() / p.len().max(1) as f64
}

fn evaluate(p: &[f64], y: &[bool], n_tr: usize) -> EvalMetrics {
    let n = p.len();
    if n == 0 { return EvalMetrics { n_train: n_tr, n_test: 0, base_rate: 0.0, accuracy: 0.0, auc: 0.5, brier: 0.0, log_loss: 0.0 }; }
    let base = y.iter().filter(|&&b| b).count() as f64 / n as f64;
    let correct = p.iter().zip(y.iter()).filter(|(pi, &yi)| (**pi > 0.5) == yi).count();
    let acc = correct as f64 / n as f64;
    let br = brier(p, y);
    let ll = p.iter().zip(y.iter()).map(|(pi, &yi)| {
        let pi = pi.clamp(1e-9, 1.0 - 1e-9);
        if yi { -pi.ln() } else { -(1.0 - pi).ln() }
    }).sum::<f64>() / n as f64;
    EvalMetrics { n_train: n_tr, n_test: n, base_rate: base, accuracy: acc,
                  auc: auc_from_sorted(p, y), brier: br, log_loss: ll }
}

fn auc_from_sorted(p: &[f64], y: &[bool]) -> f64 {
    let mut pairs: Vec<(f64, bool)> = p.iter().copied().zip(y.iter().copied()).collect();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let n_pos = y.iter().filter(|&&b| b).count() as f64;
    let n_neg = y.len() as f64 - n_pos;
    if n_pos == 0.0 || n_neg == 0.0 { return 0.5; }
    let mut rank_sum = 0.0; let mut rank = 1.0; let mut i = 0;
    while i < pairs.len() {
        let mut k = i;
        while k < pairs.len() && (pairs[k].0 - pairs[i].0).abs() < 1e-12 { k += 1; }
        let count = (k - i) as f64; let avg = rank + (count - 1.0) / 2.0;
        for m in i..k { if pairs[m].1 { rank_sum += avg; } }
        rank += count; i = k;
    }
    (rank_sum - n_pos * (n_pos + 1.0) / 2.0) / (n_pos * n_neg)
}

fn bootstrap_auc_ci(p: &[f64], y: &[bool], iters: usize) -> (f64, f64) {
    if p.len() < 20 { return (0.0, 1.0); }
    let mut rng = 0x9E3779B97F4A7C15u64;
    let mut next = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let n = p.len(); let mut aucs = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut ps = Vec::with_capacity(n); let mut ls = Vec::with_capacity(n);
        for _ in 0..n { let i = (next() as usize) % n; ps.push(p[i]); ls.push(y[i]); }
        aucs.push(auc_from_sorted(&ps, &ls));
    }
    aucs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (aucs[(iters as f64 * 0.025) as usize], aucs[(iters as f64 * 0.975) as usize])
}