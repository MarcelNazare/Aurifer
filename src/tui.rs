#![allow(dead_code)]

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Sparkline},
    Frame, Terminal,
};
use std::{io, time::{Duration, Instant}};
use crate::session::SessionState;
use crate::types::*;

pub struct App {
    bars: Vec<Bar>, cfg: Config, state: SessionState,
    current_idx: usize, paused: bool, speed: f64, should_quit: bool,
    warnings: Vec<String>, equity_history: Vec<f64>,
    last_frame: Instant, accumulated: f64,
}

impl App {
    pub fn new(bars: Vec<Bar>, cfg: Config, warnings: Vec<String>,
               predictor: Option<crate::predictor::CalibratedPredictor>) -> Self {
        let state = match predictor {
            Some(p) => SessionState::with_predictor(cfg.clone(), p),
            None    => SessionState::new(cfg.clone()),
        };
        let eq = state.equity;
        Self { bars, cfg, state, current_idx: 0, paused: false, speed: 30.0,
               should_quit: false, warnings, equity_history: vec![eq],
               last_frame: Instant::now(), accumulated: 0.0 }
    }
    pub fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
        term.clear()?;
        let res = self.loop_(&mut term);
        disable_raw_mode()?;
        execute!(term.backend_mut(), LeaveAlternateScreen)?;
        term.show_cursor()?;
        res
    }
    fn reset(&mut self) {
        self.state = SessionState::new(self.cfg.clone());
        self.current_idx = 0;
        self.equity_history = vec![self.state.equity];
        self.accumulated = 0.0; self.paused = false;
    }
    fn advance(&mut self, n: usize) {
        for _ in 0..n {
            if self.current_idx >= self.bars.len() { return; }
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
    fn loop_(&mut self, term: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            term.draw(|f| self.draw(f))?;
            let now = Instant::now();
            let dt = now.duration_since(self.last_frame).as_secs_f64();
            self.last_frame = now;
            if !self.paused && !self.should_quit && self.current_idx < self.bars.len() {
                self.accumulated += dt * self.speed;
                let steps = self.accumulated.floor() as usize;
                self.accumulated -= steps as f64;
                if steps > 0 { self.advance(steps); }
            }
            if event::poll(Duration::from_millis(33))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind != KeyEventKind::Press { continue; }
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                        KeyCode::Char(' ') => self.paused = !self.paused,
                        KeyCode::Char('s') => { self.paused = true; self.advance(1); }
                        KeyCode::Char('+') | KeyCode::Char('=') => self.speed *= 2.0,
                        KeyCode::Char('-') | KeyCode::Char('_') => self.speed = (self.speed / 2.0).max(0.25),
                        KeyCode::Char('r') => self.reset(),
                        _ => {}
                    }
                }
            }
            if self.should_quit { break; }
        }
        Ok(())
    }
    fn draw(&self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::vertical([
            Constraint::Length(3), Constraint::Length(9), Constraint::Length(9),
            Constraint::Length(6), Constraint::Min(5), Constraint::Length(1),
        ]).split(area);
        self.draw_header(f, chunks[0]);
        self.draw_mid(f, chunks[1], chunks[2]);
        self.draw_equity(f, chunks[3]);
        self.draw_trades(f, chunks[4]);
        self.draw_footer(f, chunks[5]);
    }
    fn draw_header(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let bar = self.state.last_bar;
        let ts = bar.as_ref().map(|b| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(b.ts, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
        }).unwrap_or_else(|| "—".into());
        let prog = if self.bars.is_empty() { 0.0 } else { self.current_idx as f64 / self.bars.len() as f64 };
        let status = if self.current_idx >= self.bars.len() {
            Span::styled("COMPLETE", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        } else if self.paused {
            Span::styled("PAUSED  ", Style::default().fg(Color::Yellow))
        } else {
            Span::styled("RUNNING ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        };
        let line = Line::from(vec![
            Span::styled("AURIFER ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            status, Span::raw(format!("  {ts}  ")),
            Span::raw(format!("{}/{} ({:.1}%)  ", self.current_idx, self.bars.len(), prog * 100.0)),
            Span::raw(format!("speed {:.0}/s", self.speed)),
        ]);
        let mut lines = vec![line];
        for w in &self.warnings { lines.push(Line::from(Span::styled(format!("⚠ {w}"), Style::default().fg(Color::Red)))); }
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL)), area);
    }
    fn draw_mid(&self, f: &mut Frame, price_a: ratatui::layout::Rect, ind_a: ratatui::layout::Rect) {
        let h = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(ratatui::layout::Rect { x: price_a.x, y: price_a.y, width: price_a.width, height: price_a.height });
        let bar = self.state.last_bar;
        let mut pl = Vec::new();
        if let Some(b) = bar {
            pl.push(Line::from(vec![
                Span::styled("Close  ", Style::default().fg(Color::Gray)),
                Span::styled(format!("{:.2}", b.close), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]));
            pl.push(Line::from(format!("O {:.2}  H {:.2}  L {:.2}", b.open, b.high, b.low)));
        }
        if let Some(e) = &self.state.last_eval {
            pl.push(Line::from(format!("ATR {:.2}   ADX {:.1}   RSI {:.1}",
                e.atr.unwrap_or(0.0), e.adx.unwrap_or(0.0), e.rsi.unwrap_or(0.0))));
            if let Some(p) = e.prediction {
                pl.push(Line::from(vec![Span::raw("P(up) "),
                    Span::styled(format!("{:.1}%", p * 100.0), Style::default().fg(
                        if p > 0.55 { Color::Green } else if p < 0.45 { Color::Red } else { Color::DarkGray }))]));
            }
        }
        f.render_widget(Paragraph::new(pl).block(Block::default().borders(Borders::ALL).title("PRICE")), h[0]);
        let pos_lines = match &self.state.pos {
            None => vec![
                Line::from(Span::styled("FLAT", Style::default().fg(Color::DarkGray))),
                Line::from(format!("Equity  ${:.2}", self.state.equity)),
                Line::from(format!("Trades  {}", self.state.trades.len())),
                Line::from(format!("Missed  {}", self.state.missed_trades)),
            ],
            Some(p) => {
                let upnl = p.unrealized(bar.map(|b| b.close).unwrap_or(p.entry));
                let col = if upnl >= 0.0 { Color::Green } else { Color::Red };
                let mut v = vec![
                    Line::from(vec![
                        Span::styled(p.side.as_str(), Style::default().fg(
                            if p.side == Side::Long { Color::Green } else { Color::Red }).add_modifier(Modifier::BOLD)),
                        Span::raw(format!("  {}", p.kind.as_str())),
                    ]),
                    Line::from(format!("Entry {:.2}   Units {:.2}", p.entry, p.units)),
                    Line::from(format!("SL {:.2}   TP {:.2}", p.sl, p.tp)),
                    Line::from(vec![Span::raw("uPnL  "),
                        Span::styled(format!("{}{:.2}", if upnl >= 0.0 { "+" } else { "" }, upnl), Style::default().fg(col))]),
                ];
                if p.be_moved { v.push(Line::from(Span::styled("trail active", Style::default().fg(Color::Yellow)))); }
                v
            }
        };
        f.render_widget(Paragraph::new(pos_lines).block(Block::default().borders(Borders::ALL).title("POSITION")), h[1]);
        let (rsi, adx) = self.state.last_eval.as_ref()
            .map(|e| (e.rsi.unwrap_or(0.0), e.adx.unwrap_or(0.0))).unwrap_or((0.0, 0.0));
        let ic = Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Length(3)]).split(ind_a);
        f.render_widget(Gauge::default()
            .block(Block::default().title(format!("RSI {:.1}", rsi)))
            .gauge_style(Style::default().fg(if rsi > 70.0 { Color::Red } else if rsi < 30.0 { Color::Green } else { Color::Cyan }))
            .ratio((rsi / 100.0).clamp(0.0, 1.0)), ic[0]);
        f.render_widget(Gauge::default()
            .block(Block::default().title(format!("ADX {:.1}", adx)))
            .gauge_style(Style::default().fg(if adx > 25.0 { Color::Green } else { Color::DarkGray }))
            .ratio((adx / 50.0).clamp(0.0, 1.0)), ic[1]);
        let p_up = self.state.last_eval.as_ref().and_then(|e| e.prediction).unwrap_or(0.5);
        f.render_widget(Gauge::default()
            .block(Block::default().title(format!("P(up) {:.0}%", p_up * 100.0)))
            .gauge_style(Style::default().fg(if p_up > 0.55 { Color::Green } else if p_up < 0.45 { Color::Red } else { Color::DarkGray }))
            .ratio(p_up.clamp(0.0, 1.0)), ic[2]);
    }
    fn draw_equity(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let h = &self.equity_history;
        let (lo, hi) = h.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let range = (hi - lo).max(1.0);
        let scaled: Vec<u64> = h.iter().map(|v| (((v - lo) / range) * 1000.0) as u64).collect();
        f.render_widget(Sparkline::default()
            .block(Block::default().borders(Borders::ALL)
                .title(format!("EQUITY ${:.2} (min ${:.2} / max ${:.2})", self.state.equity, lo, hi)))
            .data(&scaled)
            .style(Style::default().fg(if self.state.equity >= self.cfg.initial_equity { Color::Green } else { Color::Red })), area);
    }
    fn draw_trades(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let items: Vec<ListItem> = self.state.trades.iter().rev().take(10).map(|t| {
            let col = if t.pnl >= 0.0 { Color::Green } else { Color::Red };
            let ts = chrono::DateTime::<chrono::Utc>::from_timestamp(t.exit_ts, 0)
                .map(|d| d.format("%m-%d %H:%M").to_string()).unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::raw(format!("{ts}  ")),
                Span::styled(format!("{:<5}", t.side.as_str()),
                    Style::default().fg(if t.side == Side::Long { Color::Green } else { Color::Red })),
                Span::raw(format!(" {:<9}", t.kind.as_str())),
                Span::raw(format!(" {:.2}→{:.2} ", t.entry, t.exit)),
                Span::styled(format!("{:>+8.2} ({})", t.pnl, t.reason.as_str()), Style::default().fg(col)),
            ]))
        }).collect();
        f.render_widget(List::new(items)
            .block(Block::default().borders(Borders::ALL)
                .title(format!("RECENT TRADES ({} total, {} missed)", self.state.trades.len(), self.state.missed_trades))), area);
    }
    fn draw_footer(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let t = " [Space] pause   [S] step   [+/-] speed   [R] reset   [Q] quit ";
        f.render_widget(Paragraph::new(Line::from(Span::styled(t, Style::default().fg(Color::DarkGray)))), area);
    }
}