use std::{path::PathBuf, pin::Pin, task::Poll, task::ready};

use anyhow::{Context, Result};

use async_compression::futures::bufread::ZstdDecoder;
use bmap_parser::Bmap;
use futures::{AsyncSeek, AsyncWrite, AsyncWriteExt, io::BufReader};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Gauge, Paragraph},
};

use tokio::{
    fs::OpenOptions,
    sync::watch::{self, Sender},
};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::info;

use crate::{AppPage, pages::image::ImageInfo};

enum FlashProgress {
    Starting,
    Flashing { mapped: usize, written: usize },
    Failed(anyhow::Error),
    Finished,
}

pub struct FlashPage {
    image: ImageInfo,
    watch: watch::Receiver<FlashProgress>,
}

impl FlashPage {
    pub fn new(image: ImageInfo, target: PathBuf) -> Self {
        let f_image = image.clone();
        let (tx, watch) = watch::channel(FlashProgress::Starting);
        tokio::spawn(async move {
            flash_image(f_image.path, target, tx).await;
        });
        FlashPage { image, watch }
    }

    pub fn done(&self) -> Option<Result<()>> {
        let progress = self.watch.borrow();
        match *progress {
            FlashProgress::Starting | FlashProgress::Flashing { .. } => None,
            FlashProgress::Failed(ref e) => Some(Err(anyhow::anyhow!(format!("{e:#}")))),
            FlashProgress::Finished => Some(Ok(())),
        }
    }
}

impl Widget for &FlashPage {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let progress = self.watch.borrow();
        let perc = match *progress {
            FlashProgress::Flashing {
                mapped, written, ..
            } => (written * 100 / mapped).min(99),
            FlashProgress::Starting => 0,
            FlashProgress::Failed(ref e) => {
                Paragraph::new(format!("Failure: {e:#}",))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("Flashing failed"),
                    )
                    .style(Style::default().bg(Color::Red))
                    .render(area, buf);
                return;
            }
            FlashProgress::Finished => 100,
        };

        let title = format!("Flashing: {}", self.image.path.display());
        let label = format!("Flashing: {}%", perc);
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(title))
            .gauge_style(Style::default().fg(if perc == 100 {
                Color::Green
            } else {
                Color::LightYellow
            }))
            .percent(perc as u16)
            .label(label)
            .render(area, buf);
    }
}

impl AppPage for FlashPage {}

struct ProgressMonitor<'a, W> {
    inner: W,
    mapped: usize,
    written: usize,
    progress: &'a watch::Sender<FlashProgress>,
}

impl<'a, W> ProgressMonitor<'a, W>
where
    W: AsyncWrite + Unpin,
{
    fn new(inner: W, mapped: usize, progress: &'a watch::Sender<FlashProgress>) -> Self {
        Self {
            inner,
            mapped,
            written: 0,
            progress,
        }
    }
}

impl<W> AsyncWrite for ProgressMonitor<'_, W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let written = ready!(Pin::new(&mut self.inner).poll_write(cx, buf))?;

        self.written = self.written.saturating_add(written);
        let flashing = FlashProgress::Flashing {
            written: self.written,
            mapped: self.mapped,
        };
        let _ = self.progress.send(flashing);
        Poll::Ready(Ok(written))
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

impl<W> AsyncSeek for ProgressMonitor<'_, W>
where
    W: AsyncSeek + Unpin,
{
    fn poll_seek(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        pos: std::io::SeekFrom,
    ) -> Poll<std::io::Result<u64>> {
        Pin::new(&mut self.inner).poll_seek(cx, pos)
    }
}

async fn do_flash_image(image: PathBuf, target: PathBuf, tx: &Sender<FlashProgress>) -> Result<()> {
    info!("Flashing: {} to {}", image.display(), target.display());
    // Assume there is a bmap file
    let bmap = image.clone().with_extension("bmap");

    let xml = tokio::fs::read_to_string(&bmap)
        .await
        .with_context(|| format!("Failed to read bmap file: {}", bmap.display()))?;
    let bmap = Bmap::from_xml(&xml)
        .with_context(|| format!("Failed to parse bmap file: {}", bmap.display()))?;

    let file = tokio::fs::File::open(&image)
        .await
        .with_context(|| format!("Failed to open imagefile: {}", image.display()))?;

    // Only open existing files to avoid accidentally writing to a file in /dev rather then a block
    // device
    let target = OpenOptions::new()
        .write(true)
        .open(&target)
        .await
        .with_context(|| format!("Failed to open target: {}", target.display()))?
        .compat();
    let mut progress = ProgressMonitor::new(target, bmap.total_mapped_size() as usize, tx);

    let zstd = ZstdDecoder::new(BufReader::new(file.compat()));
    let mut zstd = bmap_parser::AsyncDiscarder::new(zstd);
    bmap_parser::copy_async(&mut zstd, &mut progress, &bmap).await?;
    progress.flush().await?;

    info!("Flashing finished");

    Ok(())
}

async fn flash_image(image: PathBuf, target: PathBuf, tx: Sender<FlashProgress>) {
    let progress = match do_flash_image(image, target, &tx).await {
        Ok(()) => FlashProgress::Finished,
        Err(e) => FlashProgress::Failed(e),
    };
    let _ = tx.send(progress);
}
