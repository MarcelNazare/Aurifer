use crate::broker::IgBroker;
use crate::types::Bar;
use anyhow::Result;
use chrono::{DateTime, Utc};

pub async fn fetch_ig_bars(
    broker: &IgBroker,
    resolution: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<Bar>> {
    broker.fetch_bars(resolution, start, end).await
}

pub async fn fetch_synthetic_bars(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Bar>> {
    eprintln!("⚠️  No IG broker configured — using SYNTHETIC bars.");
    Ok(synthetic(start, end))
}

fn synthetic(start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<Bar> {
    let minutes = ((end - start).num_minutes()).max(1000) as usize;
    let mut bars = Vec::with_capacity(minutes);
    let mut price = 2000.0_f64;
    let mut seed: u64 = 0xDEADBEEFCAFE;
    let mut trend = 0.0_f64;
    for i in 0..minutes {
        seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
        let r = ((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5;
        trend = trend * 0.995 + r * 0.05;
        price += trend + r * 0.3;
        let wick = r.abs() * 0.4 + 0.05;
        let body = r * 0.15;
        bars.push(Bar {
            ts: start.timestamp() + (i as i64) * 60,
            open: price - body, high: price + wick,
            low: price - wick, close: price + body,
            volume: 50.0 + r.abs() * 200.0,
        });
    }
    bars
}