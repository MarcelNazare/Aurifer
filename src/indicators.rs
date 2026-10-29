#![allow(dead_code)]

use std::collections::VecDeque;

pub struct Wilder { period: usize, count: usize, sum: f64, value: Option<f64> }
impl Wilder {
    pub fn new(period: usize) -> Self { Self { period: period.max(1), count: 0, sum: 0.0, value: None } }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        match self.value {
            None => { self.sum += x; self.count += 1;
                if self.count >= self.period { let v = self.sum / self.period as f64; self.value = Some(v); Some(v) } else { None } }
            Some(prev) => { let v = prev + (x - prev) / self.period as f64; self.value = Some(v); Some(v) }
        }
    }
    pub fn value(&self) -> Option<f64> { self.value }
}

pub struct Ema { period: usize, alpha: f64, count: usize, sum: f64, value: Option<f64> }
impl Ema {
    pub fn new(period: usize) -> Self { let p = period.max(1);
        Self { period: p, alpha: 2.0 / (p as f64 + 1.0), count: 0, sum: 0.0, value: None } }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        match self.value {
            None => { self.sum += x; self.count += 1;
                if self.count >= self.period { let v = self.sum / self.period as f64; self.value = Some(v); Some(v) } else { None } }
            Some(prev) => { let v = prev + self.alpha * (x - prev); self.value = Some(v); Some(v) }
        }
    }
    pub fn value(&self) -> Option<f64> { self.value }
}

pub struct Sma { period: usize, window: VecDeque<f64>, sum: f64 }
impl Sma {
    pub fn new(period: usize) -> Self { Self { period: period.max(1), window: VecDeque::new(), sum: 0.0 } }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.window.push_back(x); self.sum += x;
        if self.window.len() > self.period { if let Some(v) = self.window.pop_front() { self.sum -= v; } }
        if self.window.len() < self.period { None } else { Some(self.sum / self.period as f64) }
    }
}

pub struct Rsi { gain: Wilder, loss: Wilder, prev_close: Option<f64>, value: Option<f64> }
impl Rsi {
    pub fn new(period: usize) -> Self { Self { gain: Wilder::new(period), loss: Wilder::new(period), prev_close: None, value: None } }
    pub fn update(&mut self, close: f64) -> Option<f64> {
        let prev = match self.prev_close { Some(p) => p, None => { self.prev_close = Some(close); return None; } };
        let change = close - prev; self.prev_close = Some(close);
        let (g, l) = if change >= 0.0 { (change, 0.0) } else { (0.0, -change) };
        let ag = self.gain.update(g); let al = self.loss.update(l);
        match (ag, al) {
            (Some(ag), Some(al)) => {
                let v = if al == 0.0 { 100.0 } else { let rs = ag / al; 100.0 - 100.0 / (1.0 + rs) };
                self.value = Some(v); Some(v)
            }
            _ => None,
        }
    }
    pub fn value(&self) -> Option<f64> { self.value }
}

pub struct Atr { tr: Wilder, prev_close: Option<f64>, value: Option<f64> }
impl Atr {
    pub fn new(period: usize) -> Self { Self { tr: Wilder::new(period), prev_close: None, value: None } }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = match self.prev_close { None => high - low, Some(pc) => (high - low).max((high - pc).abs()).max((low - pc).abs()) };
        self.prev_close = Some(close); let v = self.tr.update(tr); self.value = v; v
    }
    pub fn value(&self) -> Option<f64> { self.value }
}

pub struct Adx { tr: Wilder, pdm: Wilder, ndm: Wilder, dx: Wilder, prev: Option<(f64, f64, f64)>, value: Option<f64> }
impl Adx {
    pub fn new(period: usize) -> Self {
        Self { tr: Wilder::new(period), pdm: Wilder::new(period), ndm: Wilder::new(period), dx: Wilder::new(period), prev: None, value: None }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let (ph, pl, pc) = match self.prev { Some(v) => v, None => { self.prev = Some((high, low, close)); return None; } };
        self.prev = Some((high, low, close));
        let up = high - ph; let down = pl - low;
        let plus_dm = if up > down && up > 0.0 { up } else { 0.0 };
        let minus_dm = if down > up && down > 0.0 { down } else { 0.0 };
        let tr = (high - low).max((high - pc).abs()).max((low - pc).abs());
        let s_tr = self.tr.update(tr); let s_p = self.pdm.update(plus_dm); let s_n = self.ndm.update(minus_dm);
        let (s_tr, s_p, s_n) = match (s_tr, s_p, s_n) { (Some(a), Some(b), Some(c)) => (a, b, c), _ => return None };
        if s_tr <= 0.0 { return None; }
        let pdi = 100.0 * s_p / s_tr; let ndi = 100.0 * s_n / s_tr;
        let denom = pdi + ndi;
        let dx = if denom == 0.0 { 0.0 } else { 100.0 * (pdi - ndi).abs() / denom };
        let v = self.dx.update(dx); self.value = v; v
    }
    pub fn value(&self) -> Option<f64> { self.value }
}

pub struct Bollinger { period: usize, mult: f64, window: VecDeque<f64>, sum: f64 }
impl Bollinger {
    pub fn new(period: usize, mult: f64) -> Self { Self { period: period.max(2), mult, window: VecDeque::new(), sum: 0.0 } }
    pub fn update(&mut self, close: f64) -> Option<(f64, f64, f64)> {
        self.window.push_back(close); self.sum += close;
        if self.window.len() > self.period { if let Some(v) = self.window.pop_front() { self.sum -= v; } }
        if self.window.len() < self.period { return None; }
        let n = self.period as f64; let mean = self.sum / n;
        let var = self.window.iter().map(|v| { let d = v - mean; d * d }).sum::<f64>() / n;
        let sd = var.sqrt();
        Some((mean + self.mult * sd, mean, mean - self.mult * sd))
    }
}

pub struct Keltner { ema: Ema, atr: Atr, mult: f64 }
impl Keltner {
    pub fn new(period: usize, mult: f64) -> Self { Self { ema: Ema::new(period), atr: Atr::new(period), mult } }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<(f64, f64)> {
        let m = self.ema.update(close); let a = self.atr.update(high, low, close);
        match (m, a) { (Some(m), Some(a)) => Some((m + self.mult * a, m - self.mult * a)), _ => None }
    }
}

pub struct Donchian { period: usize, highs: VecDeque<f64>, lows: VecDeque<f64> }
impl Donchian {
    pub fn new(period: usize) -> Self { Self { period: period.max(1), highs: VecDeque::new(), lows: VecDeque::new() } }
    pub fn prior(&self) -> Option<(f64, f64)> {
        if self.highs.len() < self.period { return None; }
        let hi = self.highs.iter().cloned().fold(f64::MIN, f64::max);
        let lo = self.lows.iter().cloned().fold(f64::MAX, f64::min);
        Some((hi, lo))
    }
    pub fn update(&mut self, high: f64, low: f64) {
        self.highs.push_back(high); self.lows.push_back(low);
        if self.highs.len() > self.period { self.highs.pop_front(); self.lows.pop_front(); }
    }
}

pub struct LinReg { period: usize, window: VecDeque<f64> }
impl LinReg {
    pub fn new(period: usize) -> Self { Self { period: period.max(2), window: VecDeque::new() } }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.window.push_back(x);
        if self.window.len() > self.period { self.window.pop_front(); }
        if self.window.len() < self.period { return None; }
        let n = self.period as f64;
        let (mut sx, mut sy, mut sxy, mut sxx) = (0.0, 0.0, 0.0, 0.0);
        for (i, y) in self.window.iter().enumerate() { let xi = i as f64; sx += xi; sy += y; sxy += xi * y; sxx += xi * xi; }
        let denom = n * sxx - sx * sx;
        if denom.abs() < 1e-12 { return self.window.back().copied(); }
        let slope = (n * sxy - sx * sy) / denom; let intercept = (sy - slope * sx) / n;
        Some(slope * (n - 1.0) + intercept)
    }
}