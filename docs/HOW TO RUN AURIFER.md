# How to run Aurifer

The program is a single binary. Three commands to get started.

---

## 1. Build

```powershell
cargo build --release
```

Or via the Makefile:

```powershell
make build
```

Output: `target\release\aurifer.exe` (~9.8 MB, self-contained).

**First build takes ~60 seconds** (compiles all dependencies). Subsequent builds are ~2 seconds (only your code changed).

---

## 2. Configure (optional — no credentials needed)

xaus.com requires no API key, no account, no signup. The program works out of the box.

If you want to override the cache directory (defaults to `./cache/`):

```powershell
# PowerShell
$env:XAUS_CACHE_DIR = "D:\aurifer-cache"

# bash
export XAUS_CACHE_DIR=/var/cache/aurifer
```

If you want news blackout windows (optional):

```powershell
$env:QUANTGIST_API_KEY = "your-key-here"
```

Neither is required for the program to run.

---

## 3. Run

### Forecast — the primary command

```powershell
.\target\release\aurifer.exe forecast --start 2016-01-01 --end 2026-11-01 --resolution DAY
```

**First run:** ~2 seconds (fetching ~2,500 daily bars from xaus.com). **Every subsequent run:** ~1 second (cache hit).

Prints a probability for the next 5 bars, plus the model quality report:

```
═══════════════════════════════════════════════════════════
  AURIFER FORECAST
═══════════════════════════════════════════════════════════
  Window:     2016-01-01 → 2026-11-01 (DAY resolution)
  Horizon:    5 DAY bars, threshold 0.25× ATR
  ...
  P(up):          54.7%
  Direction:      UNCERTAIN
  Confidence:     LOW
  Walk-forward:   ⚠️  UNVALIDATED
  AUC:            0.474 (95% CI 0.431–0.515)
  ...
```

Read the `Walk-forward` line first. `⚠️ UNVALIDATED` means the model has no demonstrated edge — the prediction is for research, not trading.

### Backtest — strategy performance report

```powershell
.\target\release\aurifer.exe backtest --start 2020-01-01 --end 2026-11-01 --resolution DAY
```

Runs the five strategy families on the historical data and prints trades, win rate, PnL, and the significance report.

To test **only the rule-based strategies** (RSI, breakout, squeeze, order-flow — no ML):

```powershell
.\target\release\aurifer.exe backtest --no-predictor --start 2020-01-01 --end 2026-11-01 --resolution DAY
```

### Interactive TUI

```powershell
.\target\release\aurifer.exe --resolution DAY
```

Or via `tui`:

```powershell
.\target\release\aurifer.exe tui --start 2020-01-01 --end 2026-11-01 --resolution DAY
```

Opens a full-screen terminal dashboard. Controls:

| Key | Action |
|---|---|
| `Space` | Pause / resume |
| `S` | Step one bar |
| `+` / `=` | Speed up ×2 |
| `-` / `_` | Slow down ÷2 |
| `R` | Reset to start |
| `Q` / `Esc` | Quit |

### Web dashboard

```powershell
.\target\release\aurifer.exe web --resolution DAY
```

Opens a browser to `http://localhost:<random-port>` with an equity curve, current position, indicator gauges, and a trade log. The port is random each run (printed at startup).

To suppress the browser launch (headless server, CI):

```powershell
.\target\release\aurifer.exe web --resolution DAY --no-open
```

### Diagnostics — point-prediction proof

```powershell
.\target\release\aurifer.exe diagnostics point-prediction --start 2020-01-01 --end 2026-11-01 --horizon 30
```

Proves that point prediction is untradeable. Prints an error analysis showing the model is off by 65–200× the spread. **The output will say `❌ NOT TRADEABLE` — that's the point.**

### Live — not supported

```powershell
.\target\release\aurifer.exe live
# Error: Live trading is not supported with xaus.com.
```

xaus.com is a data source, not a broker. This command deliberately errors until you wire a broker client.

---

## Recommended sequence

If you want to explore the tool for the first time:

**Step 1 — Run the forecast and read the output carefully.**

```powershell
.\target\release\aurifer.exe forecast --start 2016-01-01 --end 2026-11-01 --resolution DAY
```

Look at these three lines in order:

1. `Walk-forward: ⚠️ UNVALIDATED` — tells you the model has no proven edge
2. `AUC: 0.474 (95% CI 0.431–0.515)` — the quantitative confirmation
3. `P(up): 54.7%` — the prediction, which you should not trade on given the above

**Step 2 — Test the rule-based strategies separately.**

```powershell
.\target\release\aurifer.exe backtest --no-predictor --start 2020-01-01 --end 2026-11-01 --resolution DAY
```

This disables the ML predictor and runs only the four rule-based families. The output will tell you whether RSI/breakout/squeeze/order-flow have edge on their own.

**Step 3 — Open the TUI or web dashboard to see the strategy logic running.**

```powershell
.\target\release\aurifer.exe web --resolution DAY
```

The dashboard shows every indicator, every signal, and every trade in real time. Useful for understanding what the strategies are actually doing bar-by-bar.

**Step 4 — Read `docs/VALIDATION.md` §8** for the worked example of the current failing result, and what to try next.

---

## Command reference

| Command | What it does |
|---|---|
| `aurifer forecast` | Print one prediction for the current bar |
| `aurifer backtest` | Run strategies over a historical range, print a full report |
| `aurifer tui` | Interactive terminal dashboard (default if no subcommand) |
| `aurifer web` | Browser dashboard |
| `aurifer diagnostics point-prediction` | Prove point prediction is untradeable |
| `aurifer live` | ❌ Errors — no broker integration |

## Flags

| Flag | Applies to | Meaning |
|---|---|---|
| `--start YYYY-MM-DD` | all | Start of the history window (default: 90 days before `end`) |
| `--end YYYY-MM-DD` | all | End of the history window (default: now) |
| `--resolution DAY` | all | Bar size: `MINUTE`, `MINUTE_5`, `MINUTE_15`, `MINUTE_30`, `HOUR`, `DAY`, `WEEK` |
| `--horizon N` | forecast, backtest | Predict N bars ahead (default: 5) |
| `--label-atr-mult X` | forecast, backtest | Label threshold as fraction of ATR (default: 0.25) |
| `--no-predictor` | backtest, tui, web | Skip ML; run rule-based families only |
| `--no-cache` | all | Force a fresh fetch (bypass cache) |
| `--no-news` | all | Skip news blackout fetch |
| `--no-open` | web | Don't launch the browser |

---

## Resolution matters

**For multi-year history, always use `--resolution DAY`.**

xaus.com proxies Yahoo Finance, which limits intraday data:

| Resolution | Historical depth | Good for |
|---|---|---|
| `MINUTE` | ~7 days | Very recent intraday analysis |
| `MINUTE_5` | ~60 days | Recent intraday backtests |
| `HOUR` | ~2 years | Medium-term strategies |
| **`DAY`** | **10+ years** | **Multi-year backtests and forecasts** |
| `WEEK` | Multi-decade | Long-term trend analysis |

If you pass `--resolution MINUTE --start 2020-01-01`, you'll get an error — Yahoo doesn't have 5 years of 1-minute data. Use `DAY` for anything spanning more than a few months.

---

## Common issues

**`Error: no usable bars for 2020-01-01 → 2020-06-01 at interval=1m`**
→ You asked for 1-minute data over 6 months. Yahoo only has ~7 days. Use `--resolution DAY` or a shorter range.

**`Error: only 2500 bars available — need at least 2500`**
→ The forecast requires 10 years of daily data for walk-forward. Use `--start 2016-01-01` or earlier.

**`⚠️ LABEL IMBALANCE: base rate 72.4%`**
→ The label threshold is too easy to hit. Try `--label-atr-mult 0.40` (fewer positives) or `0.15` (more positives).

**`Walk-forward: ⚠️ UNVALIDATED`** — this is the expected result, not an error. The program correctly reports that the model has no demonstrated edge.

**Cache keeps getting stale**
→ The cache is keyed by `(resolution, start, end)`. If you use different `--start`/`--end` on each run, you get a cache miss. Pass **identical** dates for repeat runs.

**Delete cache to force a fresh fetch:**

```powershell
Remove-Item .\cache\*.json -ErrorAction SilentlyContinue
```

---

## What to expect from the current model

On 10 years of daily gold data with the default 5-day horizon, the predictor produces:

```
Holdout AUC:   0.474 (95% CI 0.431–0.515)   ← below 0.50 = no edge
Brier:         0.2522                        ← worse than always guessing 0.5
Walk-forward:  UNVALIDATED
```

This is the honest answer. Gold direction at 5-day horizons with these features is not predictable. The program's value is in telling you that clearly, not in producing a prediction you can trade on.

Before trusting any prediction, see `docs/VALIDATION.md` §8 for what this result means and what to try next (longer horizons, rule-based strategies, regime filtering).

---

## One-line summary

**`cargo build --release` then `.\target\release\aurifer.exe forecast --start 2016-01-01 --end 2026-11-01 --resolution DAY` — no credentials needed, first run takes 2 seconds, every subsequent run is instant from cache.**