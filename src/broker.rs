#![allow(dead_code)]

use crate::types::{Bar, Side};
use anyhow::{Context, Result};
use ig_client::prelude::*;

pub struct IgBroker {
    client: Client,
    epic: String,
    size: f64,
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

        Ok(Self { client, epic, size })
    }

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

    pub async fn fetch_bars(&self, resolution: &str, count: usize) -> Result<Vec<Bar>> {
        let minutes_back = count as i64 * resolution_minutes(resolution);
        let now = chrono::Utc::now();
        let from = now - chrono::Duration::minutes(minutes_back);
        let from_str = from.format("%Y-%m-%d %H:%M:%S").to_string();
        let to_str   = now.format("%Y-%m-%d %H:%M:%S").to_string();

        let resp = self
            .client
            .get_historical_prices(&self.epic, resolution, &from_str, &to_str)
            .await
            .with_context(|| {
                format!("IG get_historical_prices({resolution}, {from_str} → {to_str}) failed")
            })?;

        let mut bars = Vec::with_capacity(resp.prices.len());
        for p in &resp.prices {
            let open  = p.open_price.bid.unwrap_or(0.0);
            let high  = p.high_price.bid.unwrap_or(0.0);
            let low   = p.low_price.bid.unwrap_or(0.0);
            let close = p.close_price.bid.unwrap_or(0.0);
            let ts = parse_ig_time(&p.snapshot_time);
            if open <= 0.0 || close <= 0.0 { continue; }
            bars.push(Bar { ts, open, high, low, close, volume: 1.0 });
        }
        bars.sort_by_key(|b| b.ts);
        Ok(bars)
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
            self.epic.clone(),
            direction,
            size,
            None,
            None,
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

    /// No-op. Close positions manually via IG web/app. See docs/LIVE_TRADING.md.
    pub async fn close_all(&self) -> Result<()> {
        eprintln!("⚠️ close_all(): no-op. Close positions manually via IG web/app.");
        Ok(())
    }

    pub async fn search(&self, term: &str) -> Result<Vec<(String, String)>> {
        let res = self.client.search_markets(term).await?;
        Ok(res.markets.into_iter().map(|m| (m.epic, m.instrument_name)).collect())
    }
}

fn resolution_minutes(res: &str) -> i64 {
    match res {
        "SECOND"    => 1,
        "MINUTE"    => 1,
        "MINUTE_2"  => 2,
        "MINUTE_3"  => 3,
        "MINUTE_5"  => 5,
        "MINUTE_10" => 10,
        "MINUTE_15" => 15,
        "MINUTE_30" => 30,
        "HOUR"      => 60,
        "HOUR_2"    => 120,
        "HOUR_3"    => 180,
        "HOUR_4"    => 240,
        "DAY"       => 1440,
        "WEEK"      => 10080,
        "MONTH"     => 43200,
        _           => 1,
    }
}

fn parse_ig_time(s: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M:%S")
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
}