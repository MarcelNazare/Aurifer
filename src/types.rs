#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side { Long, Short }

impl Side {
    pub fn dir(self) -> f64 { match self { Side::Long => 1.0, Side::Short => -1.0 } }
    pub fn as_str(self) -> &'static str { match self { Side::Long => "LONG", Side::Short => "SHORT" } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyKind { Squeeze, Prediction, Breakout, OrderFlow, Rsi }

impl StrategyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StrategyKind::Squeeze    => "SQUEEZE",
            StrategyKind::Prediction => "PREDICTION",
            StrategyKind::Breakout   => "BREAKOUT",
            StrategyKind::OrderFlow  => "ORDERFLOW",
            StrategyKind::Rsi        => "RSI",
        }
    }
    pub fn priority(self) -> u8 {
        match self {
            StrategyKind::Squeeze    => 0,
            StrategyKind::Prediction => 1,
            StrategyKind::Breakout   => 2,
            StrategyKind::OrderFlow  => 3,
            StrategyKind::Rsi        => 4,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Bar {
    pub ts: i64, pub open: f64, pub high: f64, pub low: f64, pub close: f64, pub volume: f64,
}

impl Bar { pub fn range(&self) -> f64 { (self.high - self.low).max(1e-12) } }

#[derive(Debug, Clone, Copy)]
pub struct Signal { pub side: Side, pub kind: StrategyKind }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason { TakeProfit, StopLoss, TrailingStop, TimeStop, PartialTakeProfit, EndOfData }

impl ExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ExitReason::TakeProfit        => "TP",
            ExitReason::StopLoss          => "SL",
            ExitReason::TrailingStop      => "TRAIL",
            ExitReason::TimeStop          => "TIME",
            ExitReason::PartialTakeProfit => "PARTIAL",
            ExitReason::EndOfData         => "EOD",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub side: Side, pub kind: StrategyKind,
    pub entry_ts: i64, pub exit_ts: i64,
    pub entry: f64, pub exit: f64, pub units: f64, pub pnl: f64,
    pub reason: ExitReason, pub bars_held: usize,
}

#[derive(Debug, Clone)]
pub struct Position {
    pub side: Side, pub kind: StrategyKind,
    pub entry: f64, pub units: f64, pub sl: f64, pub tp: f64,
    pub entry_ts: i64, pub entry_bar: usize,
    pub extreme: f64, pub be_moved: bool, pub taken_partial: bool,
}

impl Position {
    pub fn unrealized(&self, price: f64) -> f64 {
        self.units * (price - self.entry) * self.side.dir()
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub initial_equity: f64, pub risk_per_trade: f64, pub daily_loss_limit: f64,
    pub max_trades_per_day: usize, pub max_leverage: f64,
    pub spread: f64, pub max_spread: f64, pub slippage: f64, pub commission_per_unit: f64,
    pub atr_period: usize, pub atr_sl_mult: f64, pub atr_tp_mult: f64,
    pub trail_after_atr: f64, pub trail_atr_mult: f64, pub partial_atr_mult: f64,
    pub max_bars_in_trade: usize, pub max_seconds_in_trade: i64,
    pub rsi_period: usize, pub rsi_low: f64, pub rsi_high: f64,
    pub adx_period: usize, pub adx_min: f64,
    pub ema_fast: usize, pub ema_slow: usize, pub trend_ema_period: usize,
    pub donchian_period: usize, pub bb_period: usize, pub bb_mult: f64,
    pub kc_period: usize, pub kc_mult: f64,
    pub squeeze_mom_period: usize, pub squeeze_lookback: usize,
    pub vol_sma_period: usize, pub min_delta_ratio: f64, pub min_vol_ratio: f64,
    pub required_stacked: i32,
    pub require_squeeze_gate: bool, pub require_orderflow_confirm: bool,
    pub session_start_hour: u32, pub session_end_hour: u32,
    pub trade_weekends: bool, pub news_blackouts: Vec<(i64, i64)>,
    pub bars_per_year: f64,
    pub data_source_is_mid_price: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            initial_equity: 10_000.0, risk_per_trade: 0.005, daily_loss_limit: 0.02,
            max_trades_per_day: 8, max_leverage: 10.0,
            spread: 0.30, max_spread: 0.50, slippage: 0.05, commission_per_unit: 0.0,
            atr_period: 14, atr_sl_mult: 1.5, atr_tp_mult: 3.0,
            trail_after_atr: 1.0, trail_atr_mult: 1.0, partial_atr_mult: 1.5,
            max_bars_in_trade: 30, max_seconds_in_trade: 1800,
            rsi_period: 14, rsi_low: 30.0, rsi_high: 70.0,
            adx_period: 14, adx_min: 20.0,
            ema_fast: 13, ema_slow: 34, trend_ema_period: 200,
            donchian_period: 10, bb_period: 20, bb_mult: 2.0,
            kc_period: 20, kc_mult: 1.5,
            squeeze_mom_period: 12, squeeze_lookback: 3,
            vol_sma_period: 20, min_delta_ratio: 0.60, min_vol_ratio: 1.30,
            required_stacked: 2,
            require_squeeze_gate: false, require_orderflow_confirm: false,
            session_start_hour: 7, session_end_hour: 17,
            trade_weekends: false, news_blackouts: Vec::new(),
            bars_per_year: 60.0 * 24.0 * 252.0,
            data_source_is_mid_price: true,
        }
    }
}