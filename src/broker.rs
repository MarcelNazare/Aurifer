#![allow(dead_code)]

use crate::types::{Bar, Side};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ig_client::prelude::*;
use std::path::PathBuf;

pub struct IgBroker {
    client: Client,
    epic: String,
    size: f64,
    cache_dir: PathBuf,
    allow_no_cache: bool,
}

impl IgBroker {
    pub async fn connect() -> Result<Self> {
        let client = Client::try_new()
            .map_err(|e| anyhow::anyhow!("IG login failed: {e}"))?;

        client
            .get_accounts()
            .await
            .context("IG get_accounts failed — check IG_* in .env")?;

        let epic = std::env::var("IG_GOLD_EPIC")
            .unwrap_or_else(|_| "CS.D.CFDGOLD.CFDGC.IP".to_string());
        let size = std::env::var("IG_ORDER_SIZE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0);

        let cache_dir = PathBuf::from(
            std::env::var("IG_CACHE_DIR").unwrap_or_else(|_| "./cache".to_string())
        );
        let _ = std::fs::create_dir_all(&cache_dir);

        Ok(Self { client, epic, size, cache_dir, allow_no_cache: false })
    }

    pub fn set_no_cache(&mut self, v: bool) { self.allow_no_cache = v; }
    pub fn epic(&self) -> &str { &self.epic }
    pub fn size(&self) -> f64 { self.size }

    pub async fn snapshot(&self) -> Result<(f64, f64)> {
        let details = self
            .client
            .get_market_details(&self.epic)
            .await
            .context("IG get_market_details failed")?;

        let bid = details
            .snapshot
            .bid
            .ok_or_else(|| anyhow::anyhow!("IG snapshot: no bid price"))?;
        let offer = details
            .snapshot
            .offer
            .ok_or_else(|| anyhow::anyhow!("IG snapshot: no offer price"))?;

        Ok((bid, offer))
    }

    /// Fetch historical bars with local disk caching.
    ///
    /// IG's `/prices` endpoint defaults to 20 points per request when
    /// `max` is unspecified, and `ig-client 0.18` does not expose `max`.
    /// We work within the 20-point window: each chunk = 20 × resolution.
    /// IG quota is measured in points, not requests, so 108 tiny requests
    /// cost the same quota as 3 big ones.
    pub async fn fetch_bars(
        &self,
        resolution: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<Bar>> {
        const POINTS_PER_REQUEST: i64 = 20;

        let cache_path = self.cache_path(resolution, start, end);

        // ── 1. Cache lookup ──────────────────────────────────────
        if !self.allow_no_cache && cache_path.exists() {
            if let Ok(text) = std::fs::read_to_string(&cache_path) {
                if let Ok(bars) = serde_json::from_str::<Vec<CachedBar>>(&text) {
                    let out: Vec<Bar> = bars.into_iter().map(|c| c.into()).collect();
                    if !out.is_empty() {
                        println!("  📦 cache hit: {} bars from {}",
                                 out.len(), cache_path.display());
                        return Ok(out);
                    }
                }
            }
        }

        // ── 2. Pre-flight estimate ───────────────────────────────
        let res_minutes = resolution_minutes(resolution).max(1);
        let total_minutes = (end - start).num_minutes().max(0);
        let est_points = (total_minutes / res_minutes).max(1) as usize;
        let est_requests = (est_points + POINTS_PER_REQUEST as usize - 1)
            / POINTS_PER_REQUEST as usize;
        let weekly_budget = 10_000usize;

        println!("  estimated {} data points, ~{} requests (weekly budget: {})",
                 est_points, est_requests, weekly_budget);

        if est_points > weekly_budget {
            let needed_days = (est_points as f64 / weekly_budget as f64 * 7.0).ceil() as i64;
            anyhow::bail!(
                "range needs {} points, weekly allowance is {}. \
                 Narrow to ≤{} days, or use --resolution HOUR.",
                est_points, weekly_budget, needed_days);
        }

        // ── 3. Fetch in 20-point chunks ──────────────────────────
        let chunk = chrono::Duration::minutes(POINTS_PER_REQUEST * res_minutes);
        let mut bars: Vec<Bar> = Vec::new();
        let mut cursor = start;
        let mut request_count = 0usize;
        let mut raw_price_count = 0usize;
        let mut first_sample: Option<String> = None;
        let mut last_progress_pct: i32 = -1;

        while cursor < end {
            let chunk_end = (cursor + chunk).min(end);
            let from_str = cursor.format("%Y-%m-%dT%H:%M:%S").to_string();
            let to_str   = chunk_end.format("%Y-%m-%dT%H:%M:%S").to_string();

            let done_minutes = (cursor - start).num_minutes().max(0);
            let pct = if total_minutes > 0 {
                (done_minutes as f64 / total_minutes as f64 * 100.0) as i32
            } else { 100 };
            if pct / 10 > last_progress_pct / 10 || request_count == 0 {
                println!("  progress: {}% ({} bars so far, {} requests)",
                         pct, bars.len(), request_count);
                last_progress_pct = pct;
            }

            let resp = self
                .client
                .get_historical_prices(&self.epic, resolution, &from_str, &to_str)
                .await
                .map_err(|e| {
                    let msg = e.to_string();
                    if msg.contains("allowance") || msg.contains("exceeded") {
                        anyhow::anyhow!(
                            "IG weekly allowance exhausted at {} → {}. Quota resets Monday 00:01.",
                            from_str, to_str)
                    } else {
                        anyhow::anyhow!(
                            "IG get_historical_prices({resolution}, {from_str} → {to_str}) failed: {e}")
                    }
                })?;

            request_count += 1;
            let chunk_raw = resp.prices.len();
            raw_price_count += chunk_raw;

            if chunk_raw > 0 && first_sample.is_none() {
                if let Some(p) = resp.prices.first() {
                    first_sample = Some(format!(
                        "snapshotTime={:?} open_bid={:?} close_bid={:?}",
                        p.snapshot_time, p.open_price.bid, p.close_price.bid));
                }
            }

            for p in &resp.prices {
                let open  = p.open_price.bid.unwrap_or(0.0);
                let high  = p.high_price.bid.unwrap_or(0.0);
                let low   = p.low_price.bid.unwrap_or(0.0);
                let close = p.close_price.bid.unwrap_or(0.0);
                let ts = parse_ig_time(&p.snapshot_time);
                if open <= 0.0 || close <= 0.0 || ts == 0 { continue; }
                bars.push(Bar { ts, open, high, low, close, volume: 1.0 });
            }

            cursor = chunk_end;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }

        bars.sort_by_key(|b| b.ts);
        bars.dedup_by_key(|b| b.ts);

        println!("  processed {} bars from {} raw prices in {} request(s)",
                 bars.len(), raw_price_count, request_count);

        if let Some(s) = first_sample {
            println!("  first price sample: {}", s);
        }

        // ── 4. Empty result handling ────────────────────────────
        if bars.is_empty() {
            let today = Utc::now();
            let hint_start = (today - chrono::Duration::days(30))
                .format("%Y-%m-%d").to_string();
            let hint_end = today.format("%Y-%m-%d").to_string();

            if raw_price_count == 0 {
                anyhow::bail!(
                    "IG returned 0 prices for {} → {} ({} requests, all empty).\n\
                     Try a recent range:\n\
                       aurifer backtest --start {} --end {}\n\
                     Or run with no --start/--end for a 90-day default.",
                    start.format("%Y-%m-%d"), end.format("%Y-%m-%d"),
                    request_count, hint_start, hint_end);
            } else {
                anyhow::bail!(
                    "IG returned {} raw prices for {} → {} but none had usable OHLC.",
                    raw_price_count, start.format("%Y-%m-%d"), end.format("%Y-%m-%d"));
            }
        }

        // ── 5. Write cache ──────────────────────────────────────
        if !self.allow_no_cache {
            let cached: Vec<CachedBar> = bars.iter().map(|b| CachedBar::from(*b)).collect();
            if let Ok(json) = serde_json::to_string(&cached) {
                let _ = std::fs::write(&cache_path, json);
                println!("  💾 cached to {}", cache_path.display());
            }
        }

        Ok(bars)
    }

    fn cache_path(&self, resolution: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> PathBuf {
        let safe_epic = self.epic.replace(|c: char| !c.is_alphanumeric(), "_");
        let fname = format!(
            "{}_{}_{}_{}.json",
            safe_epic, resolution,
            start.format("%Y%m%dT%H%M"),
            end.format("%Y%m%dT%H%M"),
        );
        self.cache_dir.join(fname)
    }

    pub async fn place_market_order(
        &self,
        side: Side,
        size: f64,
        stop_loss: Option<f64>,
        take_profit: Option<f64>,
    ) -> Result<String> {
        let direction = match side {
            Side::Long  => Direction::Buy,
            Side::Short => Direction::Sell,
        };

        let mut req = CreateOrderRequest::market(
            self.epic.clone(), direction, size, None, None,
        );
        if let Some(sl) = stop_loss   { req = req.with_stop_loss(sl); }
        if let Some(tp) = take_profit { req = req.with_take_profit(tp); }

        let result = self
            .client
            .create_order(&req)
            .await
            .context("IG create_order failed")?;
        Ok(result.deal_reference)
    }

    pub async fn close_all(&self) -> Result<()> {
        eprintln!("⚠️ close_all(): no-op. Close positions manually via IG web/app.");
        Ok(())
    }

    pub async fn search(&self, term: &str) -> Result<Vec<(String, String)>> {
        let res = self.client.search_markets(term).await?;
        Ok(res.markets.into_iter().map(|m| (m.epic, m.instrument_name)).collect())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedBar {
    ts: i64, open: f64, high: f64, low: f64, close: f64, volume: f64,
}
impl From<Bar> for CachedBar {
    fn from(b: Bar) -> Self {
        Self { ts: b.ts, open: b.open, high: b.high, low: b.low, close: b.close, volume: b.volume }
    }
}
impl From<CachedBar> for Bar {
    fn from(c: CachedBar) -> Self {
        Self { ts: c.ts, open: c.open, high: c.high, low: c.low, close: c.close, volume: c.volume }
    }
}

fn resolution_minutes(res: &str) -> i64 {
    match res {
        "SECOND"    => 1, "MINUTE" => 1, "MINUTE_2" => 2, "MINUTE_3" => 3,
        "MINUTE_5"  => 5, "MINUTE_10" => 10, "MINUTE_15" => 15, "MINUTE_30" => 30,
        "HOUR"      => 60, "HOUR_2" => 120, "HOUR_3" => 180, "HOUR_4" => 240,
        "DAY"       => 1440, "WEEK" => 10080, "MONTH" => 43200, _ => 1,
    }
}

fn parse_ig_time(s: &str) -> i64 {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return dt.timestamp();
    }
    0
}