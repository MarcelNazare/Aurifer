use crate::broker::IgBroker;
use crate::types::Bar;
use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use reqwest::Client;
use std::path::PathBuf;

const CSV_SOURCES: &[(&str, &str)] = &[
    ("GitHub (FeziweMelvin)",
     "https://raw.githubusercontent.com/FeziweMelvin/XAUUSD-Gold-Price/main/XAU_1d_data.csv"),
    ("Zenodo (academic)",
     "https://zenodo.org/records/18774339/files/daily_ohlcv_clean.csv?download=1"),
    ("Stooq (XAUUSD)", "https://stooq.com/q/d/l/?s=xauusd&i=d"),
    ("Stooq (Gold futures)", "https://stooq.com/q/d/l/?s=gc.f&i=d"),
];

const XAUS_HISTORY_URL: &str = "https://xaus.com/api/v1/history";
const XAUS_SPOT_URL: &str = "https://xaus.com/api/v1/spot?compact=1";

pub async fn fetch_ig_bars(
    broker: &IgBroker, resolution: &str,
    start: DateTime<Utc>, end: DateTime<Utc>,
) -> Result<Vec<Bar>> {
    broker.fetch_bars(resolution, start, end).await
}

pub async fn fetch_synthetic_bars(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Bar>> {
    eprintln!("⚠️  No IG broker — using SYNTHETIC bars (meaningless)");
    Ok(synthetic(start, end))
}

pub async fn fetch_csv_bars(
    resolution: &str, start: DateTime<Utc>, end: DateTime<Utc>,
) -> Result<Vec<Bar>> {
    let upper = resolution.to_uppercase();
    if upper != "DAY" && upper != "WEEK" && upper != "MONTH" {
        anyhow::bail!("CSV source only supports DAY/WEEK/MONTH, not {}", resolution);
    }

    let cache_dir = PathBuf::from(
        std::env::var("AURIFER_CACHE_DIR").unwrap_or_else(|_| "./cache".to_string()));
    let _ = std::fs::create_dir_all(&cache_dir);
    let cache_path = cache_dir.join("xauusd_daily.csv");

    let csv_text = load_csv_text(&cache_path).await?;
    let mut bars = process_csv(&csv_text, resolution, start, end)?;
    println!("  CSV: {} bars ({} → {})",
             bars.len(),
             ts_to_date(bars.first().map(|b| b.ts).unwrap_or(0)),
             ts_to_date(bars.last().map(|b| b.ts).unwrap_or(0)));

    let last_csv_ts = bars.last().map(|b| b.ts).unwrap_or(0);
    let today_ts = Utc::now().timestamp();
    if today_ts - last_csv_ts > 2 * 86400 {
        println!("  ⚠️  CSV ends {} — fetching recent daily bars from xaus.com …", ts_to_date(last_csv_ts));
        match fetch_xaus_history().await {
            Ok(recent) => {
                let fresh: Vec<Bar> = recent.into_iter().filter(|b| b.ts > last_csv_ts).collect();
                if !fresh.is_empty() {
                    println!("  xaus history: merged {} bars ({} → {})",
                             fresh.len(),
                             ts_to_date(fresh.first().unwrap().ts),
                             ts_to_date(fresh.last().unwrap().ts));
                    bars.extend(fresh);
                }
            }
            Err(e) => eprintln!("  ⚠️  xaus history fetch failed: {e}"),
        }
    }

    match fetch_xaus_spot().await {
        Ok((spot_price, spot_ts)) if spot_price > 0.0 => {
            let last_ts = bars.last().map(|b| b.ts).unwrap_or(0);
            if spot_ts > last_ts {
                let open = bars.last().map(|b| b.close).unwrap_or(spot_price);
                bars.push(Bar { ts: spot_ts, open, high: open.max(spot_price),
                                low: open.min(spot_price), close: spot_price, volume: 1.0 });
                println!("  live spot: ${:.2} appended for {}", spot_price, ts_to_date(spot_ts));
            } else if let Some(last) = bars.last_mut() {
                last.high = last.high.max(spot_price);
                last.low = last.low.min(spot_price);
                last.close = spot_price;
                println!("  live spot: ${:.2} applied to last bar", spot_price);
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("  ⚠️  xaus spot fetch failed: {e}"),
    }

    bars.sort_by_key(|b| b.ts);
    bars.dedup_by_key(|b| b.ts);
    bars.retain(|b| b.ts >= start.timestamp() && b.ts <= end.timestamp());

    println!("  final: {} {} bars ({} → {})",
             bars.len(), resolution,
             ts_to_date(bars.first().map(|b| b.ts).unwrap_or(0)),
             ts_to_date(bars.last().map(|b| b.ts).unwrap_or(0)));

    if bars.is_empty() { anyhow::bail!("no bars after merging"); }
    Ok(bars)
}

async fn load_csv_text(cache_path: &PathBuf) -> Result<String> {
    if let Ok(p) = std::env::var("AURIFER_CSV") {
        let path = PathBuf::from(&p);
        if path.exists() {
            println!("  📄 loading CSV from AURIFER_CSV: {}", path.display());
            return std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("could not read AURIFER_CSV {}: {}", path.display(), e));
        } else {
            eprintln!("  ⚠️  AURIFER_CSV set to {} but missing — downloading", path.display());
        }
    }

    if cache_path.exists() {
        let text = std::fs::read_to_string(cache_path)?;
        if validate_csv(&text).is_ok() {
            println!("  📦 CSV cache hit: {}", cache_path.display());
            return Ok(text);
        } else {
            eprintln!("  ⚠️  cached CSV invalid — re-downloading");
            let _ = std::fs::remove_file(cache_path);
        }
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("Mozilla/5.0 (compatible; Aurifer/0.1)")
        .build()?;

    let mut errors: Vec<String> = Vec::new();
    for (name, url) in CSV_SOURCES {
        println!("  ⬇️  trying {} …", name);
        match download_csv(&client, url).await {
            Ok(text) if validate_csv(&text).is_ok() => {
                let _ = std::fs::write(cache_path, &text);
                println!("  💾 cached CSV");
                return Ok(text);
            }
            Ok(_) => errors.push(format!("{}: invalid CSV", name)),
            Err(e) => errors.push(format!("{}: {}", name, e)),
        }
    }

    eprintln!();
    eprintln!("❌ ALL CSV SOURCES FAILED:");
    for e in &errors { eprintln!("  • {}", e); }
    eprintln!();
    eprintln!("MANUAL FIX — download to: {}", cache_path.display());
    eprintln!("  https://stooq.com/q/d/l/?s=xauusd&i=d");
    anyhow::bail!("no CSV source available")
}

fn validate_csv(text: &str) -> Result<()> {
    let head = text.lines().take(5).collect::<Vec<_>>().join("\n").to_lowercase();
    if head.contains("<!doctype") || head.contains("<html") { anyhow::bail!("HTML, not CSV"); }
    if head.contains("git-lfs") { anyhow::bail!("Git LFS pointer"); }
    if text.len() < 1000 { anyhow::bail!("file too small"); }
    Ok(())
}

async fn download_csv(client: &Client, url: &str) -> Result<String> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() { anyhow::bail!("HTTP {}", resp.status()); }
    let text = resp.text().await?;
    validate_csv(&text)?;
    Ok(text)
}

async fn fetch_xaus_history() -> Result<Vec<Bar>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (compatible; Aurifer/0.1)")
        .build()?;
    let resp = client.get(XAUS_HISTORY_URL).send().await?;
    if !resp.status().is_success() { anyhow::bail!("HTTP {}", resp.status()); }
    let json: serde_json::Value = resp.json().await?;
    let points = json["points"].as_array().ok_or_else(|| anyhow!("no 'points' array"))?;
    let mut bars: Vec<Bar> = Vec::with_capacity(points.len());
    let mut prev_close: Option<f64> = None;
    for p in points {
        let d = p["d"].as_str().unwrap_or("");
        let ts = parse_date(d).unwrap_or(0);
        if ts == 0 { continue; }
        let close = p["c"].as_f64().unwrap_or(0.0);
        let high = p["h"].as_f64().unwrap_or(close);
        let low = p["l"].as_f64().unwrap_or(close);
        let open = prev_close.unwrap_or(close);
        if close <= 0.0 { continue; }
        bars.push(Bar { ts, open, high: high.max(open).max(close),
                        low: low.min(open).min(close), close, volume: 1.0 });
        prev_close = Some(close);
    }
    Ok(bars)
}

async fn fetch_xaus_spot() -> Result<(f64, i64)> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (compatible; Aurifer/0.1)")
        .build()?;
    let resp = client.get(XAUS_SPOT_URL).send().await?;
    if !resp.status().is_success() { anyhow::bail!("HTTP {}", resp.status()); }
    let json: serde_json::Value = resp.json().await?;
    if let Some(state) = json.get("data_state") {
        if state["status"].as_str() == Some("unavailable") {
            anyhow::bail!("xaus spot unavailable");
        }
    }
    let price = json["spot_usd_oz"].as_f64()
        .or_else(|| json["xau"]["price"].as_f64())
        .ok_or_else(|| anyhow!("no spot_usd_oz"))?;
    let day_start = Utc::now().date_naive().and_hms_opt(0, 0, 0).unwrap()
        .and_utc().timestamp();
    Ok((price, day_start))
}

fn process_csv(text: &str, resolution: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Bar>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for delim in &[',', ';', '\t', '|'] {
        match try_parse_with_delimiter(text, *delim) {
            Ok(bars) if !bars.is_empty() => {
                println!("  parsed {} rows (delim {:?})", bars.len(), delim);
                let mut bars = bars;
                bars.retain(|b| b.ts >= start.timestamp() && b.ts <= end.timestamp());
                bars.sort_by_key(|b| b.ts);
                bars.dedup_by_key(|b| b.ts);
                let upper = resolution.to_uppercase();
                let bars = match upper.as_str() {
                    "DAY" => bars,
                    "WEEK" => resample(&bars, 7 * 24 * 3600),
                    "MONTH" => resample(&bars, 30 * 24 * 3600),
                    _ => bars,
                };
                return Ok(bars);
            }
            _ => continue,
        }
    }
    let preview = text.lines().take(5).collect::<Vec<_>>().join("\n");
    anyhow::bail!("CSV parsed 0 rows. First 5:\n{}", preview)
}

fn try_parse_with_delimiter(text: &str, delim: char) -> Result<Vec<Bar>> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().ok_or_else(|| anyhow!("empty"))?;
    let first_lower = first.to_lowercase();
    let has_header = first_lower.contains("date") || first_lower.contains("open") || first_lower.contains("time");
    let mut bars = Vec::new();
    if !has_header {
        if let Some(b) = parse_row(first, delim) { bars.push(b); }
    }
    for line in lines {
        if line.trim().starts_with('#') { continue; }
        if let Some(b) = parse_row(line, delim) { bars.push(b); }
    }
    Ok(bars)
}

fn parse_row(line: &str, delim: char) -> Option<Bar> {
    let fields: Vec<&str> = line.split(delim).map(|s| s.trim()).collect();
    if fields.len() < 5 { return None; }
    let ts = parse_date(fields[0])?;
    let open: f64 = fields[1].parse().ok()?;
    let high: f64 = fields[2].parse().ok()?;
    let low: f64 = fields[3].parse().ok()?;
    let close: f64 = fields[4].parse().ok()?;
    let volume: f64 = fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    if open <= 0.0 || close <= 0.0 { return None; }
    Some(Bar { ts, open, high, low, close, volume })
}

fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let normalized = s.replace('.', "-");
    for fmt in &[
        "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M",
        "%Y/%m/%d %H:%M:%S", "%Y/%m/%d %H:%M", "%m/%d/%Y %H:%M:%S", "%m/%d/%Y %H:%M",
        "%d/%m/%Y %H:%M:%S", "%d/%m/%Y %H:%M",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(&normalized, fmt) { return Some(ndt.and_utc().timestamp()); }
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) { return Some(ndt.and_utc().timestamp()); }
    }
    for fmt in &["%Y-%m-%d", "%Y/%m/%d", "%m/%d/%Y", "%d/%m/%Y"] {
        if let Ok(nd) = NaiveDate::parse_from_str(&normalized, fmt) {
            return nd.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc().timestamp());
        }
        if let Ok(nd) = NaiveDate::parse_from_str(s, fmt) {
            return nd.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc().timestamp());
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) { return Some(dt.timestamp()); }
    None
}

fn resample(bars: &[Bar], bucket_secs: i64) -> Vec<Bar> {
    let mut out = Vec::new();
    let mut cur: Option<Bar> = None;
    for b in bars {
        let bucket = b.ts - b.ts.rem_euclid(bucket_secs);
        match cur.as_mut() {
            None => cur = Some(Bar { ts: bucket, ..*b }),
            Some(c) if c.ts == bucket => {
                c.high = c.high.max(b.high);
                c.low = c.low.min(b.low);
                c.close = b.close;
                c.volume += b.volume;
            }
            Some(_) => {
                out.push(cur.take().unwrap());
                cur = Some(Bar { ts: bucket, ..*b });
            }
        }
    }
    if let Some(c) = cur { out.push(c); }
    out
}

fn ts_to_date(ts: i64) -> String {
    if ts == 0 { return "—".into(); }
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "—".into())
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
        trend = trend * 0.80 + r * 0.20;
        price += trend * 0.3 + r * 0.8;
        let wick = r.abs() * 0.6 + 0.05;
        let body = r * 0.2;
        bars.push(Bar {
            ts: start.timestamp() + (i as i64) * 60,
            open: price - body, high: price + wick,
            low: price - wick, close: price + body,
            volume: 50.0 + r.abs() * 200.0,
        });
    }
    bars
}