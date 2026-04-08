use std::process::Command;

use crossterm::event::{Event, KeyCode};
use ratatui::widgets::Borders;
use ratatui::{
    prelude::*,
    widgets::{Block, Paragraph},
};
use tracing::{error, info};

use crate::AppPage;

pub struct FinalPage {
    result: anyhow::Result<()>,
}

impl FinalPage {
    pub fn new(result: anyhow::Result<()>) -> Self {
        Self { result }
    }
}

impl Widget for &FinalPage {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        if let Err(e) = &self.result {
            Paragraph::new(format!("Failure: {e:#}\nPress enter to reboot",))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Flashing failed"),
                )
                .style(Style::default().bg(Color::Red))
                .render(area, buf);
        } else {
            Paragraph::new("Done!\nPress enter to reboot")
                .block(Block::default().borders(Borders::ALL).title("Huge success"))
                .style(Style::default().bg(Color::Green))
                .render(area, buf);
        }
    }
}

impl AppPage for FinalPage {
    fn input(&mut self, event: crossterm::event::Event) {
        let Event::Key(k) = event else { return };
        if let KeyCode::Enter = k.code {
            info!("Rebooting!");
            match Command::new("reboot").output() {
                Ok(out) => info!(
                    "stdout: {}, stderr: {}, status: {}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr),
                    out.status
                ),
                Err(e) => error!("Failed to run reboot: {e:#}"),
            }
        }
    }
}
