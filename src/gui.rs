#![allow(dead_code)]

use anyhow::Result;
use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use std::time::{Duration, Instant};

use crate::session::SessionState;
use crate::types::*;

pub struct App {
    bars: Vec<Bar>,
    cfg: Config,
    state: SessionState,
    current_idx: usize,
    paused: bool,
    speed: f64,
    warnings: Vec<String>,
    equity_history: Vec<f64>,
    accumulated: f64,
    last_frame: Instant,
    forecast: Option<ForecastSummary>,
    news_events: Vec<NewsEvent>,
}

impl App {
    pub fn new(
        bars: Vec<Bar>,
        cfg: Config,
        warnings: Vec<String>,
        predictor: Option<crate::predictor::CalibratedPredictor>,
        forecast: Option<ForecastSummary>,
        news_events: Vec<NewsEvent>,
    ) -> Self {
        let state = match predictor {
            Some(p) => SessionState::with_predictor(cfg.clone(), p),
            None => SessionState::new(cfg.clone()),
        };
        let eq = state.equity;
        Self {
            bars,
            cfg,
            state,
            current_idx: 0,
            paused: false,
            speed: 30.0,
            warnings,
            equity_history: vec![eq],
            accumulated: 0.0,
            last_frame: Instant::now(),
            forecast,
            news_events,
        }
    }

    fn reset(&mut self) {
        self.state = SessionState::new(self.cfg.clone());
        self.current_idx = 0;
        self.equity_history = vec![self.state.equity];
        self.accumulated = 0.0;
        self.paused = false;
    }

    fn advance(&mut self, n: usize) {
        for _ in 0..n {
            if self.current_idx >= self.bars.len() {
                return;
            }
            self.state.step(&self.bars[self.current_idx]);
            self.current_idx += 1;
            self.equity_history.push(self.state.equity);
        }
        if self.current_idx >= self.bars.len() {
            if let Some(last) = self.bars.last().copied() {
                self.state.force_close(&last);
                self.equity_history.push(self.state.equity);
                self.paused = true;
            }
        }
    }

    fn sim_now(&self) -> i64 {
        self.state
            .last_bar
            .map(|b| b.ts)
            .or_else(|| self.bars.first().map(|b| b.ts))
            .unwrap_or_else(|| chrono::Utc::now().timestamp())
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Space) {
                self.paused = !self.paused;
            }
            if i.key_pressed(egui::Key::S) {
                self.paused = true;
                self.advance(1);
            }
            if i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus) {
                self.speed *= 2.0;
            }
            if i.key_pressed(egui::Key::Minus) {
                self.speed = (self.speed / 2.0).max(0.25);
            }
            if i.key_pressed(egui::Key::R) {
                self.reset();
            }
            if i.key_pressed(egui::Key::Q) || i.key_pressed(egui::Key::Escape) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;
        if !self.paused && self.current_idx < self.bars.len() {
            self.accumulated += dt * self.speed;
            let steps = self.accumulated.floor() as usize;
            self.accumulated -= steps as f64;
            if steps > 0 {
                self.advance(steps);
            }
            ctx.request_repaint_after(Duration::from_millis(33));
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| self.draw_header(ui));

        if self.forecast.is_some() {
            egui::TopBottomPanel::top("forecast").show(ctx, |ui| self.draw_forecast(ui));
        }

        if !self.news_events.is_empty() {
            egui::TopBottomPanel::top("news").show(ctx, |ui| self.draw_news(ui));
        }

        egui::TopBottomPanel::bottom("controls").show(ctx, |ui| self.draw_controls(ui));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(3, |cols| {
                ui_group(&mut cols[0], "PRICE", |ui| self.draw_price(ui));
                ui_group(&mut cols[1], "POSITION", |ui| self.draw_position(ui));
                ui_group(&mut cols[2], "INDICATORS", |ui| self.draw_indicators(ui));
            });
            ui.separator();
            self.draw_equity(ui);
            ui.separator();
            self.draw_trades(ui);
        });
    }
}

impl App {
    fn draw_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("AURIFER");

            let (status, color) = if self.current_idx >= self.bars.len() {
                ("COMPLETE", egui::Color32::YELLOW)
            } else if self.paused {
                ("PAUSED", egui::Color32::YELLOW)
            } else {
                ("RUNNING", egui::Color32::GREEN)
            };
            ui.colored_label(color, egui::RichText::new(status).strong());

            let ts = self
                .state
                .last_bar
                .as_ref()
                .map(|b| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(b.ts, 0)
                        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_else(|| "—".into());
            ui.label(ts);

            let prog = if self.bars.is_empty() {
                0.0
            } else {
                self.current_idx as f64 / self.bars.len() as f64
            };
            ui.label(format!(
                "{}/{} ({:.1}%)",
                self.current_idx,
                self.bars.len(),
                prog * 100.0
            ));
            ui.label(format!("speed {:.0}/s", self.speed));
        });

        for w in &self.warnings {
            ui.colored_label(egui::Color32::RED, format!("⚠ {w}"));
        }
    }

    fn draw_forecast(&self, ui: &mut egui::Ui) {
        let Some(fc) = &self.forecast else { return };

        ui.heading("FORECAST");
        ui.label(format!(
            "next {} {} bars · as of {} @ ${:.2}",
            fc.horizon, fc.resolution, fc.last_date, fc.last_close
        ));

        let dir_color = match fc.direction.as_str() {
            "LIKELY UP" => egui::Color32::GREEN,
            "LIKELY DOWN" => egui::Color32::RED,
            _ => egui::Color32::YELLOW,
        };
        ui.horizontal(|ui| {
            ui.label("P(up)");
            ui.colored_label(
                dir_color,
                egui::RichText::new(format!("{:.1}%", fc.p_up * 100.0)).strong(),
            );
            ui.label("→");
            ui.colored_label(dir_color, egui::RichText::new(&fc.direction).strong());
            ui.label(format!(
                "({} confidence, edge {:.1}%)",
                fc.confidence, fc.edge_pct
            ));
        });
        ui.horizontal(|ui| {
            ui.label("AUC:");
            ui.colored_label(
                if fc.auc >= 0.54 { egui::Color32::GREEN } else { egui::Color32::YELLOW },
                format!("{:.3}", fc.auc),
            );
            ui.label(format!("[{:.3}–{:.3}]", fc.auc_ci_lo, fc.auc_ci_hi));
            ui.label("Brier:");
            ui.colored_label(
                if fc.brier < 0.24 { egui::Color32::GREEN } else { egui::Color32::RED },
                format!("{:.4}", fc.brier),
            );
            ui.label(format!("base rate: {:.1}%", fc.base_rate * 100.0));
        });
        ui.horizontal(|ui| {
            ui.label("Walk-forward:");
            if fc.validated {
                ui.colored_label(egui::Color32::GREEN, "✅ VALIDATED");
            } else {
                ui.colored_label(egui::Color32::YELLOW, "⚠️ UNVALIDATED");
            }
            ui.label("· top:");
            ui.colored_label(egui::Color32::GRAY, fc.features_line());
        });
    }

    fn draw_news(&self, ui: &mut egui::Ui) {
        let now = self.sim_now();

        let mut active: Vec<&NewsEvent> = Vec::new();
        let mut upcoming: Vec<&NewsEvent> = Vec::new();
        let mut recent: Vec<&NewsEvent> = Vec::new();
        for e in &self.news_events {
            let (lo, hi) = e.blackout();
            if lo <= now && now <= hi {
                active.push(e);
            } else if e.ts > now {
                upcoming.push(e);
            } else if e.ts > now - 3600 {
                recent.push(e);
            }
        }
        upcoming.sort_by_key(|e| e.ts);
        upcoming.truncate(4);
        recent.sort_by_key(|e| -e.ts);
        recent.truncate(2);

        let mut header = String::from("NEWS");
        if !active.is_empty() {
            header.push_str(&format!("  ⏰ {} active blackout", active.len()));
        }
        ui.horizontal(|ui| {
            ui.heading(header);
            if !upcoming.is_empty() {
                ui.label(format!("· next {} upcoming", upcoming.len()));
            }
        });

        let draw_line = |ui: &mut egui::Ui, e: &NewsEvent, active: bool| {
            let when = chrono::DateTime::<chrono::Utc>::from_timestamp(e.ts, 0)
                .map(|d| d.format("%b %d %H:%M").to_string())
                .unwrap_or_default();
            let rel = fmt_relative(now, e.ts);
            let color = if active {
                egui::Color32::RED
            } else if e.ts > now {
                egui::Color32::LIGHT_BLUE
            } else {
                egui::Color32::GRAY
            };
            let tag_color = match e.kind {
                NewsKind::Fomc => egui::Color32::YELLOW,
                NewsKind::News => egui::Color32::LIGHT_GRAY,
            };
            ui.horizontal(|ui| {
                ui.colored_label(tag_color, format!("[{}]", e.kind.as_str()));
                ui.colored_label(color, format!("{:<16}", when));
                ui.colored_label(color, format!("{:<10}", rel));
                ui.label(&e.title);
            });
        };

        for e in active {
            draw_line(ui, e, true);
        }
        for e in recent {
            draw_line(ui, e, false);
        }
        for e in upcoming {
            draw_line(ui, e, false);
        }
    }

    fn draw_controls(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("[Space] pause   [S] step   [+/-] speed   [R] reset   [Q] quit");
        });
    }

    fn draw_price(&self, ui: &mut egui::Ui) {
        if let Some(b) = self.state.last_bar {
            ui.label(egui::RichText::new(format!("Close {:.2}", b.close)).strong());
            ui.label(format!("O {:.2}  H {:.2}  L {:.2}", b.open, b.high, b.low));
        }
        if let Some(e) = &self.state.last_eval {
            ui.label(format!(
                "ATR {:.2}   ADX {:.1}   RSI {:.1}",
                e.atr.unwrap_or(0.0),
                e.adx.unwrap_or(0.0),
                e.rsi.unwrap_or(0.0)
            ));
            if let Some(p) = e.prediction {
                let col = if p > 0.55 {
                    egui::Color32::GREEN
                } else if p < 0.45 {
                    egui::Color32::RED
                } else {
                    egui::Color32::GRAY
                };
                ui.colored_label(col, format!("P(up) {:.1}%", p * 100.0));
            }
        }
    }

    fn draw_position(&self, ui: &mut egui::Ui) {
        match &self.state.pos {
            None => {
                ui.colored_label(egui::Color32::GRAY, "FLAT");
                ui.label(format!("Equity  ${:.2}", self.state.equity));
                ui.label(format!("Trades  {}", self.state.trades.len()));
                ui.label(format!("Missed  {}", self.state.missed_trades));
            }
            Some(p) => {
                let upnl = p.unrealized(
                    self.state.last_bar.map(|b| b.close).unwrap_or(p.entry),
                );
                let col = if upnl >= 0.0 {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                };
                let side_col = if p.side == Side::Long {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(
                    side_col,
                    egui::RichText::new(format!("{} {}", p.side.as_str(), p.kind.as_str()))
                        .strong(),
                );
                ui.label(format!("Entry {:.2}   Units {:.2}", p.entry, p.units));
                ui.label(format!("SL {:.2}   TP {:.2}", p.sl, p.tp));
                ui.colored_label(col, format!("uPnL {:+.2}", upnl));
                if p.be_moved {
                    ui.colored_label(egui::Color32::YELLOW, "trail active");
                }
            }
        }
    }

    fn draw_indicators(&self, ui: &mut egui::Ui) {
        let (rsi, adx) = self
            .state
            .last_eval
            .as_ref()
            .map(|e| (e.rsi.unwrap_or(0.0), e.adx.unwrap_or(0.0)))
            .unwrap_or((0.0, 0.0));
        let p_up = self
            .state
            .last_eval
            .as_ref()
            .and_then(|e| e.prediction)
            .unwrap_or(0.5);

        let rsi_col = if rsi > 70.0 {
            egui::Color32::RED
        } else if rsi < 30.0 {
            egui::Color32::GREEN
        } else {
            egui::Color32::LIGHT_BLUE
        };
        ui.add(
            egui::ProgressBar::new((rsi / 100.0) as f32)
                .text(format!("RSI  {:.1}", rsi))
                .fill(rsi_col),
        );

        let adx_col = if adx > 25.0 {
            egui::Color32::GREEN
        } else {
            egui::Color32::GRAY
        };
        ui.add(
            egui::ProgressBar::new((adx / 50.0).clamp(0.0, 1.0) as f32)
                .text(format!("ADX  {:.1}", adx))
                .fill(adx_col),
        );

        let p_col = if p_up > 0.55 {
            egui::Color32::GREEN
        } else if p_up < 0.45 {
            egui::Color32::RED
        } else {
            egui::Color32::GRAY
        };
        ui.add(
            egui::ProgressBar::new(p_up as f32)
                .text(format!("P(up) {:.0}%", p_up * 100.0))
                .fill(p_col),
        );
    }

    fn draw_equity(&self, ui: &mut egui::Ui) {
        let h = &self.equity_history;
        let (lo, hi) = h
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));

        ui.heading(format!(
            "EQUITY ${:.2}  (min ${:.2} / max ${:.2})",
            self.state.equity, lo, hi
        ));

        let color = if self.state.equity >= self.cfg.initial_equity {
            egui::Color32::GREEN
        } else {
            egui::Color32::RED
        };
        let points: Vec<[f64; 2]> = h
            .iter()
            .enumerate()
            .map(|(i, v)| [i as f64, *v])
            .collect();
        let line = Line::new(PlotPoints::from(points)).color(color);

        Plot::new("equity_plot")
            .height(160.0)
            .show_axes([true, true])
            .show(ui, |plot_ui| {
                plot_ui.line(line);
            });
    }

    fn draw_trades(&self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "RECENT TRADES ({} total, {} missed)",
            self.state.trades.len(),
            self.state.missed_trades
        ));

        egui::ScrollArea::vertical()
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("trades_grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong("TIME");
                        ui.strong("SIDE");
                        ui.strong("KIND");
                        ui.strong("ENTRY→EXIT");
                        ui.strong("PnL");
                        ui.end_row();

                        for t in self.state.trades.iter().rev().take(50) {
                            let ts =
                                chrono::DateTime::<chrono::Utc>::from_timestamp(t.exit_ts, 0)
                                    .map(|d| d.format("%m-%d %H:%M").to_string())
                                    .unwrap_or_default();
                            ui.label(ts);

                            let side_col = if t.side == Side::Long {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            };
                            ui.colored_label(side_col, t.side.as_str());
                            ui.label(t.kind.as_str());
                            ui.label(format!("{:.2}→{:.2}", t.entry, t.exit));

                            let pnl_col = if t.pnl >= 0.0 {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            };
                            ui.colored_label(
                                pnl_col,
                                format!("{:+.2} ({})", t.pnl, t.reason.as_str()),
                            );
                            ui.end_row();
                        }
                    });
            });
    }
}

fn ui_group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.heading(title);
        add(ui);
    });
}

fn fmt_relative(now: i64, ts: i64) -> String {
    let d = ts - now;
    let abs = d.abs();
    let (n, unit) = if abs < 60 {
        (abs, "s")
    } else if abs < 3600 {
        (abs / 60, "m")
    } else if abs < 86_400 {
        (abs / 3600, "h")
    } else {
        (abs / 86_400, "d")
    };
    if d >= 0 {
        format!("in {}{}", n, unit)
    } else {
        format!("{}{} ago", n, unit)
    }
}

pub fn run_gui(
    bars: Vec<Bar>,
    cfg: Config,
    warnings: Vec<String>,
    predictor: Option<crate::predictor::CalibratedPredictor>,
    forecast: Option<ForecastSummary>,
    news_events: Vec<NewsEvent>,
) -> Result<()> {
    let app = App::new(bars, cfg, warnings, predictor, forecast, news_events);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 900.0])
            .with_min_inner_size([900.0, 640.0])
            .with_title("Aurifer"),
        ..Default::default()
    };
    eframe::run_native(
        "Aurifer",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;
    Ok(())
}