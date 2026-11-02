#![allow(dead_code)]

use crate::types::{Bar, Side};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use ig_client::prelude::*;
use std::path::PathBuf;

pub struct IgBroker {
    client: Client,
    epic: String,
    size: f64,
    cache_dir: PathBuf,
    allow_no_cache: bool,
    weekly_point_budget: usize,
}

impl IgBroker {
    pub async fn connect() -> Result<Self> {
        let client = Client::try_new().map_err(|e| anyhow!("IG login failed: {e}"))?;
        client.get_accounts().await.context("IG get_accounts failed — check IG_* in .env")?;

        let epic = std::env::var("IG_GOLD_EPIC").unwrap_or_else(|_| "CS.D.CFDGOLD.CFDGC.IP".to_string());
        let size = std::env::var("IG_ORDER_SIZE").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
        let cache_dir = PathBuf::from(std::env::var("IG_CACHE_DIR").unwrap_or_else(|_| "./cache".to_string()));
        let _ = std::fs::create_dir_all(&cache_dir);
        let weekly_point_budget = std::env::var("IG_WEEKLY_DATA_POINT_BUDGET")
            .ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(10_000);

        Ok(Self { client, epic, size, cache_dir, allow_no_cache: false, weekly_point_budget })
    }

    pub fn set_no_cache(&mut self, v: bool) { self.allow_no_cache = v; }
    pub fn epic(&self) -> &str { &self.epic }
    pub fn size(&self) -> f64 { self.size }

    pub async fn snapshot(&self) -> Result<(f64, f64)> {
        let details = self.client.get_market_details(&self.epic).await
            .context("IG get_market_details failed")?;
        let bid = details.snapshot.bid.ok_or_else(|| anyhow!("IG snapshot: no bid price"))?;
        let offer = details.snapshot.offer.ok_or_else(|| anyhow!("IG snapshot: no offer price"))?;
        Ok((bid, offer))
    }

    pub async fn fetch_bars(&self, resolution: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Bar>> {
        let cache_path = self.cache_path(resolution, start, end);
        if !self.allow_no_cache && cache_path.exists() {
            if let Ok(text) = std::fs::read_to_string(&cache_path) {
                if let Ok(bars) = serde_json::from_str::<Vec<CachedBar>>(&text) {
                    let out: Vec<Bar> = bars.into_iter().map(|c| c.into()).collect();
                    if !out.is_empty() {
                        println!("  📦 cache hit: {} bars (0 points used)", out.len());
                        return Ok(out);
                    }
                }
            }
        }

        let res_minutes = resolution_minutes(resolution).max(1);
        let total_minutes = (end - start).num_minutes().max(0);
        let est_points = (total_minutes / res_minutes).max(1) as usize;
        println!("  📊 estimated {} data points (weekly budget: {})", est_points, self.weekly_point_budget);

        if est_points > self.weekly_point_budget {
            let max_days = (self.weekly_point_budget as i64) * res_minutes / (60 * 24);
            anyhow::bail!(
                "QUOTA EXCEEDED: range needs {} points, weekly allowance is {}.\n\
                 Narrow the range to ≤{} days at {} resolution.\n\
                 Note: 10-year DAY ≈ 2,500 points, 3-month HOUR ≈ 2,160 points — both fit.",
                est_points, self.weekly_point_budget, max_days, resolution);
        }

        let chunk = chrono::Duration::minutes(1000 * res_minutes);
        let mut bars: Vec<Bar> = Vec::new();
        let mut cursor = start;
        let mut request_count = 0usize;

        while cursor < end {
            let chunk_end = (cursor + chunk).min(end);
            let from_str = cursor.format("%Y-%m-%dT%H:%M:%S").to_string();
            let to_str = chunk_end.format("%Y-%m-%dT%H:%M:%S").to_string();
            let resp = self.client.get_historical_prices(&self.epic, resolution, &from_str, &to_str).await
                .map_err(|e| {
                    let msg = e.to_string();
                    if msg.contains("allowance") || msg.contains("exceeded") {
                        anyhow!("IG weekly allowance exhausted at {} → {}", from_str, to_str)
                    } else {
                        anyhow!("IG get_historical_prices({}) failed: {e}", resolution)
                    }
                })?;
            request_count += 1;
            for p in &resp.prices {
                let open = p.open_price.bid.unwrap_or(0.0);
                let high = p.high_price.bid.unwrap_or(0.0);
                let low = p.low_price.bid.unwrap_or(0.0);
                let close = p.close_price.bid.unwrap_or(0.0);
                let ts = parse_ig_time(&p.snapshot_time);
                if open <= 0.0 || close <= 0.0 || ts == 0 { continue; }
                bars.push(Bar { ts, open, high, low, close, volume: 1.0 });
            }
            cursor = chunk_end;
        }

        bars.sort_by_key(|b| b.ts);
        bars.dedup_by_key(|b| b.ts);
        println!("  ✅ {} bars in {} request(s)", bars.len(), request_count);

        if bars.is_empty() {
            anyhow::bail!("IG returned no usable bars");
        }
        if !self.allow_no_cache {
            let cached: Vec<CachedBar> = bars.iter().map(|b| CachedBar::from(*b)).collect();
            if let Ok(json) = serde_json::to_string(&cached) {
                let _ = std::fs::write(&cache_path, json);
                println!("  💾 cached");
            }
        }
        Ok(bars)
    }

    pub async fn place_market_order(
        &self, side: Side, size: f64,
        stop_loss: Option<f64>, take_profit: Option<f64>,
    ) -> Result<String> {
        let direction = match side { Side::Long => Direction::Buy, Side::Short => Direction::Sell };
        let mut req = CreateOrderRequest::market(self.epic.clone(), direction, size, None, None);
        if let Some(sl) = stop_loss { req = req.with_stop_loss(sl); }
        if let Some(tp) = take_profit { req = req.with_take_profit(tp); }
        let result = self.client.create_order(&req).await.context("IG create_order failed")?;
        Ok(result.deal_reference)
    }

    fn cache_path(&self, resolution: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> PathBuf {
        let safe_epic = self.epic.replace(|c: char| !c.is_alphanumeric(), "_");
        let fname = format!("{}_{}_{}_{}.json", safe_epic, resolution,
                            start.format("%Y%m%dT%H%M"), end.format("%Y%m%dT%H%M"));
        self.cache_dir.join(fname)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedBar { ts: i64, open: f64, high: f64, low: f64, close: f64, volume: f64 }

impl From<Bar> for CachedBar {
    fn from(b: Bar) -> Self { Self { ts: b.ts, open: b.open, high: b.high, low: b.low, close: b.close, volume: b.volume } }
}
impl From<CachedBar> for Bar {
    fn from(c: CachedBar) -> Self { Self { ts: c.ts, open: c.open, high: c.high, low: c.low, close: c.close, volume: c.volume } }
}

fn resolution_minutes(res: &str) -> i64 {
    match res.to_uppercase().as_str() {
        "SECOND" => 1, "MINUTE" => 1, "MINUTE_2" => 2, "MINUTE_3" => 3,
        "MINUTE_5" => 5, "MINUTE_10" => 10, "MINUTE_15" => 15, "MINUTE_30" => 30,
        "HOUR" => 60, "HOUR_2" => 120, "HOUR_3" => 180, "HOUR_4" => 240,
        "DAY" => 1440, "WEEK" => 10080, "MONTH" => 43200, _ => 1,
    }
}

fn parse_ig_time(s: &str) -> i64 {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) { return dt.timestamp(); }
    0
}