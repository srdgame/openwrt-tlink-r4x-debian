use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;
use std::{io::stdout, path::PathBuf, time::Duration};
use tokio::time::{self};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use zbus_systemd::{systemd1::ManagerProxy, zbus};

use crossterm::{
    ExecutableCommand,
    event::{Event, KeyCode, KeyModifiers},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    prelude::CrosstermBackend,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Widget},
};

mod pages;
use crate::pages::image::ImagePage;
use crate::pages::{r#final::FinalPage, flash::FlashPage};

enum Page {
    // Page to pick the image
    Image(ImagePage),
    Flash(FlashPage),
    Final(FinalPage),
}

impl Page {
    async fn next_step(&mut self, options: &Opts) {
        match self {
            Page::Image(i) => {
                if let Some(image) = i.selected() {
                    info!("Selected!: {}", image.path.display());
                    let target = if options.fake {
                        let _ = tokio::fs::File::create("test.img").await;
                        PathBuf::from("test.img")
                    } else {
                        // Evil hardcoded of NVME device
                        PathBuf::from("/dev/nvme0n1")
                    };
                    let f = FlashPage::new(image, target);
                    *self = Self::Flash(f);
                }
            }
            Page::Flash(f) => {
                if let Some(done) = f.done() {
                    *self = Self::Final(FinalPage::new(done));
                }
            }
            Page::Final(_) => (),
        }
    }
}

trait AppPage {
    fn input(&mut self, _event: Event) {}
    async fn needs_update(&mut self) {
        std::future::pending::<()>().await;
    }
}

impl AppPage for Page {
    fn input(&mut self, event: Event) {
        match self {
            Page::Image(i) => i.input(event),
            Page::Flash(f) => f.input(event),
            Page::Final(f) => f.input(event),
        }
    }

    async fn needs_update(&mut self) {
        match self {
            Page::Image(i) => i.needs_update().await,
            Page::Flash(f) => f.needs_update().await,
            Page::Final(f) => f.needs_update().await,
        };
    }
}

impl Widget for &Page {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        match self {
            Page::Image(i) => i.render(area, buf),
            Page::Flash(f) => f.render(area, buf),
            Page::Final(f) => f.render(area, buf),
        }
    }
}

struct App<B: ratatui::backend::Backend> {
    terminal: Terminal<B>,
    page: Page,
    options: Opts,
}

impl<B> App<B>
where
    B: ratatui::backend::Backend,
    B::Error: Send + Sync + 'static,
{
    fn new(terminal: Terminal<B>, options: Opts) -> Self {
        Self {
            terminal,
            page: Page::Image(ImagePage::new(options.images.clone())),
            options,
        }
    }

    async fn run(&mut self) -> Result<()> {
        let mut reader = crossterm::event::EventStream::new();
        loop {
            self.draw()?;
            let event = reader.next();
            // Force redraw every 250ms to update e.g logging
            let delay = time::sleep(Duration::from_millis(250));
            let needs_update = self.page.needs_update();
            tokio::select! {
                event = event =>  {
                    if let Some(Ok(e)) = event {
                        if let Event::Key(k) = e
                            && k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL) {
                                return Ok(());
                        }
                        self.page.input(e);
                    }

                }
                _ = delay => {}
                _ = needs_update => {}
            };
            self.page.next_step(&self.options).await;
        }
    }

    fn draw(&mut self) -> Result<()> {
        self.terminal.draw(|frame| {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Percentage(60),
                    Constraint::Percentage(40),
                ])
                .split(frame.area());

            let line = Line::from("Openwrt one flasher -- ^C to exit")
                .style(Style::default().bg(Color::Black).fg(Color::Red))
                .centered();
            frame.render_widget(line, layout[0]);
            frame.render_widget(&self.page, layout[1]);
            frame.render_widget(
                tui_logger::TuiLoggerWidget::default().block(Block::bordered()),
                layout[2],
            );
        })?;
        Ok(())
    }
}

async fn shutup_printk() {
    // If this fails simply ignore
    let _ = tokio::fs::write("/proc/sys/kernel/printk", "1\n").await;
}

#[derive(Parser, Debug, Clone)]
struct Opts {
    /// Fake destructive operations e.g. flash image to disk.img
    #[clap(long)]
    fake: bool,
    // Directories to also scan for images
    #[clap(short, long)]
    images: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tui_logger::init_logger(tui_logger::LevelFilter::Trace)?;
    tui_logger::set_default_level(tui_logger::LevelFilter::Info);
    tracing_subscriber::Registry::default()
        .with(tui_logger::TuiTracingSubscriberLayer)
        .init();

    let opts = Opts::parse();
    if opts.fake {
        warn!("Running in FAKE mode");
    }

    let conn = zbus::Connection::system()
        .await
        .context("Failed to connect to system bus")?;
    let manager = ManagerProxy::new(&conn)
        .await
        .context("Failed to connect to manager proxy")?;

    shutup_printk().await;
    let current_show_status = manager.show_status().await.unwrap_or_else(|e| {
        warn!("Failed to get systemds ShowStatus: {e:#}");
        true
    });
    if let Err(e) = manager.set_show_status("false".to_string()).await {
        warn!("Failed to change systemds ShowStatus: {e:#}");
    }

    let (width, height) = crossterm::terminal::size()?;
    if width == 0 || height == 0 {
        // If size is unset, force to 80x24
        rustix::termios::tcsetwinsize(
            std::io::stdout(),
            rustix::termios::Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
    }

    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    App::new(terminal, opts).run().await?;

    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;

    if let Err(e) = manager
        .set_show_status(current_show_status.to_string())
        .await
    {
        warn!("Failed to reset systemds ShowStatus: {e:#}");
    }
    Ok(())
}
