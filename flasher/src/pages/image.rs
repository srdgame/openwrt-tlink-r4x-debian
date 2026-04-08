use std::{
    collections::HashMap,
    future::pending,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use crossterm::event::{Event, KeyCode};
use futures::StreamExt;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListState},
};
use tokio::sync::mpsc::Sender;
use tracing::{error, info, warn};
use zbus_systemd::{
    systemd1::{ManagerProxy, MountProxy},
    zbus::{self, Connection},
    zvariant::{self, ObjectPath},
};

use crate::AppPage;

#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub path: PathBuf,
}

enum Update {
    New(ImageInfo),
    Removed(PathBuf),
}

pub struct ImagePage {
    images: Vec<ImageInfo>,
    cursor: usize,
    selected: Option<ImageInfo>,
    rx: tokio::sync::mpsc::Receiver<Update>,
}

impl ImagePage {
    pub fn new(images: Vec<PathBuf>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async {
            if let Err(e) = monitor_images(tx, images).await {
                error!("Image monitor failed: {e:?}");
            }
        });
        Self {
            images: Vec::new(),
            cursor: 0,
            selected: None,
            rx,
        }
    }

    pub fn selected(&mut self) -> Option<ImageInfo> {
        self.selected.take()
    }
}

impl Widget for &ImagePage {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let mut state = ListState::default()
            .with_selected(Some(self.cursor.min(self.images.len().saturating_sub(1))));
        let list = List::new(self.images.iter().map(|i| i.path.display().to_string()))
            .block(Block::default().title("Select image").borders(Borders::ALL))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol(">>");

        StatefulWidget::render(list, area, buf, &mut state);
    }
}

impl AppPage for ImagePage {
    fn input(&mut self, event: crossterm::event::Event) {
        let Event::Key(k) = event else { return };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.images.len().saturating_sub(1))
            }
            KeyCode::Enter => self.selected = self.images.get(self.cursor).cloned(),
            _ => (),
        }
    }

    async fn needs_update(&mut self) {
        let update = self.rx.recv().await;
        if let Some(update) = update {
            match update {
                Update::New(i) => {
                    info!("New image: {:?}", i);
                    self.images.push(i);
                }

                Update::Removed(prefix) => {
                    self.images.retain(|info| !info.path.starts_with(&prefix))
                }
            }
        } else {
            pending::<()>().await;
        }
    }
}

enum MountUpdate {
    New(PathBuf),
    Removed(PathBuf),
}

// Pick up mount units if they're backed by a real devices and mounted under /run
async fn update_for_mount(conn: &Connection, obj: &ObjectPath<'_>) -> Option<PathBuf> {
    let mount = MountProxy::new(conn, obj).await.ok()?;
    let what = mount.what().await.context("Failed to get where").ok()?;
    if !what.starts_with("/dev/") {
        return None;
    }

    let where_ = mount
        .where_property()
        .await
        .context("Failed to get where")
        .ok()?;

    if !where_.starts_with("/run/") {
        return None;
    }
    Some(PathBuf::from(where_))
}

async fn monitor_mount_units(tx: Sender<MountUpdate>) -> anyhow::Result<()> {
    let mut seen = HashMap::new();
    // Monitor systemd mount units
    let conn = zbus::Connection::system()
        .await
        .context("Failed to connect to system bus")?;
    let manager = ManagerProxy::new(&conn)
        .await
        .context("Failed to connect to manager proxy")?;

    let mut new_stream = manager
        .receive_unit_new()
        .await
        .context("Failed to setup stream for receiving new units")?;
    let mut remove_stream = manager
        .receive_unit_removed()
        .await
        .context("Failed to setup stream for receiving unit removals")?;

    // Returns:
    // * The primary unit name
    // * The human-readable description string
    // * The load state (i.e. whether the unit file has been loaded successfully)
    // * The active state (i.e. whether the unit is currently started or not)
    // * The sub state (a more fine-grained version of the active state that is specific to the unit type, which the active state is not)
    // * A unit that is being followed in its state by this unit, if there is any, otherwise the empty string.
    // * The unit object path
    // * If there is a job queued for the job unit, the numeric job id, 0 otherwise
    // * The job type as string
    // * The job object path
    let units = manager
        .list_units_by_patterns(vec![], vec!["*.mount".to_string()])
        .await
        .context("Failed to get mount units")?;

    for u in units {
        if let Some(path) = update_for_mount(&conn, &u.6).await {
            seen.insert(u.6, path.clone());
            tx.send(MountUpdate::New(path)).await?;
        }
    }

    loop {
        tokio::select! {
            new = new_stream.next() => {
                let Some(new) = new else {
                    bail!("new unit stream closed");
                };
                let body = new.message().body();
                let (name, p): (&str, zvariant::ObjectPath) = body
                    .deserialize()
                    .context("Failed to deserialize new unit dbus message")?;
                // Only care about mount units
                if !name.ends_with(".mount") {
                    continue;
                }
                // Ensure this is  properly backed
                // If we just send messages to a unit object path, systemd spurious creates one which
                // causes a new signal
                if manager.get_unit(name.to_string()).await.is_ok()
                    && let Some(path) = update_for_mount(&conn, &p).await {
                        seen.insert(p.into(), path.clone());
                        tx.send(MountUpdate::New(path)).await?;
                }
            }
            removed = remove_stream.next() => {
                let Some(removed) = removed else {
                    bail!("removed unit stream closed");
                };
                let body = removed.message().body();
                let (name, p): (&str, zvariant::ObjectPath) = body
                    .deserialize()
                    .context("Failed to deserialize removed unit dbus message")?;
                if let Some(path) = seen.remove(&p) {
                        info!("removed: {} - {}", name, p);
                        tx.send(MountUpdate::Removed(path)).await?;
                }
            }
        };
    }
}

async fn scan_dir_for_images(tx: &Sender<Update>, path: &Path) {
    let mut dir = match tokio::fs::read_dir(&path).await {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read {}: {:?}", path.display(), e);
            return;
        }
    };

    while let Ok(Some(entry)) = dir.next_entry().await {
        let f = path.join(entry.file_name());
        if let Some(e) = f.extension()
            && e == "zst"
        {
            let type_ = match entry.file_type().await {
                Ok(t) => t,
                Err(e) => {
                    warn!("failed to determine file type for {}: {:?}", f.display(), e);
                    continue;
                }
            };
            if type_.is_file() && tx.send(Update::New(ImageInfo { path: f })).await.is_err() {
                break;
            }
        }
    }
}

async fn monitor_images(tx: Sender<Update>, images: Vec<PathBuf>) -> anyhow::Result<()> {
    let (mount_tx, mut mount_rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async {
        if let Err(e) = monitor_mount_units(mount_tx).await {
            warn!("Mount unit monitoring failed: {e:?}");
        }
    });

    for i in images {
        scan_dir_for_images(&tx, &i).await;
    }

    loop {
        let m = tokio::select! {
            _ = tx.closed() => {
                info!("Images closed");
                break;
            }
            update = mount_rx.recv() => {
                match update {
                    Some(m) => m,
                    None => {
                        warn!("Mount monitor failed");
                        break;
                    }
                }
            }
        };

        match m {
            MountUpdate::New(ref path) => {
                info!("Detected mountpoint: {}", path.display());
                scan_dir_for_images(&tx, path).await;
            }
            MountUpdate::Removed(path) => {
                if tx.send(Update::Removed(path)).await.is_err() {
                    break;
                }
            }
        }
    }

    Ok(())
}
