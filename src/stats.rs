#![allow(dead_code)]

use crate::types::Trade;

pub struct SampleStats {
    pub n_trades: usize, pub win_rate: f64, pub win_rate_ci: (f64, f64),
    pub breakeven_wr: f64, pub is_significant: bool,
    pub pnl_ci: (f64, f64), pub sharpe: f64, pub sharpe_ci: (f64, f64),
    pub warnings: Vec<String>,
}

pub fn analyze(trades: &[Trade], _bars_per_year: f64) -> SampleStats {
    let n = trades.len(); let mut warnings = Vec::new();
    if n == 0 {
        return SampleStats {
            n_trades: 0, win_rate: 0.0, win_rate_ci: (0.0, 1.0), breakeven_wr: 0.5,
            is_significant: false, pnl_ci: (0.0, 0.0), sharpe: 0.0, sharpe_ci: (0.0, 0.0),
            warnings: vec!["no trades".into()],
        };
    }
    let wins = trades.iter().filter(|t| t.pnl > 0.0).count();
    let win_rate = wins as f64 / n as f64;
    let z = 1.96;
    let denom = 1.0 + z * z / n as f64;
    let center = (win_rate + z * z / (2.0 * n as f64)) / denom;
    let margin = z * (win_rate * (1.0 - win_rate) / n as f64 + z * z / (4.0 * n as f64 * n as f64)).sqrt() / denom;
    let win_rate_ci = ((center - margin).max(0.0), (center + margin).min(1.0));
    let wins_p: Vec<f64> = trades.iter().filter(|t| t.pnl > 0.0).map(|t| t.pnl).collect();
    let loss_p: Vec<f64> = trades.iter().filter(|t| t.pnl <= 0.0).map(|t| -t.pnl).collect();
    let avg_win = if wins_p.is_empty() { 0.0 } else { wins_p.iter().sum::<f64>() / wins_p.len() as f64 };
    let avg_loss = if loss_p.is_empty() { 1.0 } else { loss_p.iter().sum::<f64>() / loss_p.len() as f64 };
    let breakeven_wr = if avg_win + avg_loss > 0.0 { avg_loss / (avg_win + avg_loss) } else { 0.5 };
    let is_significant = win_rate_ci.0 > breakeven_wr;
    if n < 100 { warnings.push(format!("⚠️ only {} trades — need ≥100, preferably ≥500", n)); }
    let pnls: Vec<f64> = trades.iter().map(|t| t.pnl).collect();
    let (pnl_ci, sharpe_ci) = bootstrap(&pnls, 1000);
    let sharpe = compute_sharpe(&pnls);
    if pnl_ci.0 < 0.0 && pnl_ci.1 > 0.0 {
        warnings.push("⚠️ 95% CI on total PnL crosses zero — results could be luck".into());
    }
    if !is_significant && n >= 100 {
        warnings.push(format!("⚠️ win rate 95% CI [{:.1}%, {:.1}%] does not clear breakeven {:.1}%",
                              win_rate_ci.0 * 100.0, win_rate_ci.1 * 100.0, breakeven_wr * 100.0));
    }
    if sharpe_ci.0 < 0.0 {
        warnings.push("⚠️ Sharpe 95% CI includes negative territory — edge not proven".into());
    }
    SampleStats { n_trades: n, win_rate, win_rate_ci, breakeven_wr, is_significant,
                  pnl_ci, sharpe, sharpe_ci, warnings }
}

fn compute_sharpe(pnls: &[f64]) -> f64 {
    if pnls.len() < 2 { return 0.0; }
    let mean = pnls.iter().sum::<f64>() / pnls.len() as f64;
    let var = pnls.iter().map(|p| (p - mean) * (p - mean)).sum::<f64>() / (pnls.len() - 1) as f64;
    let std = var.sqrt();
    if std < 1e-9 { return 0.0; }
    mean / std * (252.0_f64).sqrt()
}

fn bootstrap(pnls: &[f64], iters: usize) -> ((f64, f64), (f64, f64)) {
    let n = pnls.len();
    if n < 10 { return ((0.0, 0.0), (0.0, 0.0)); }
    let mut rng = 0xDEADBEEFu64;
    let mut next = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let mut totals = Vec::with_capacity(iters); let mut sharpes = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut sample = Vec::with_capacity(n);
        for _ in 0..n { let i = (next() as usize) % n; sample.push(pnls[i]); }
        totals.push(sample.iter().sum::<f64>());
        sharpes.push(compute_sharpe(&sample));
    }
    totals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sharpes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lo = (iters as f64 * 0.025) as usize;
    let hi = (iters as f64 * 0.975) as usize;
    ((totals[lo], totals[hi]), (sharpes[lo], sharpes[hi]))
}