//! D-Bus client for `org.gnome.Mutter.ScreenCast` (CreateSession / RecordVirtual /
//! PipeWireStreamAdded / Start / Stop).
//!
//! Normative flow: GTK `headless-monitor-tests.py` and Chrome Remote Desktop's
//! `gnome_remote_desktop_session.cc` (see `docs/00-feasibility.md`).

use std::collections::HashMap;
use std::time::Duration;

use async_io::Timer;
use futures_lite::{future, StreamExt};
use zbus::proxy;
use zbus::zvariant::OwnedObjectPath;
use zbus::zvariant::Value;
use zbus::Connection;

/// Cursor modes on mutter ScreenCast streams (same numbering as the portal).
pub mod cursor {
    pub const HIDDEN: u32 = 0;
    pub const EMBEDDED: u32 = 1;
    pub const METADATA: u32 = 2;
}

#[derive(Debug)]
pub enum GnomeError {
    Zbus(zbus::Error),
    Timeout(&'static str),
}

impl std::fmt::Display for GnomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GnomeError::Zbus(e) => write!(f, "dbus: {e}"),
            GnomeError::Timeout(s) => write!(f, "timeout: {s}"),
        }
    }
}

impl std::error::Error for GnomeError {}

impl From<zbus::Error> for GnomeError {
    fn from(e: zbus::Error) -> Self {
        GnomeError::Zbus(e)
    }
}

/// Parameters for a virtual-monitor ScreenCast session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    /// Treat as a real platform monitor (not a "share this screen" stream).
    pub is_platform: bool,
    pub cursor_mode: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            width: 1600,
            height: 900,
            fps: 60,
            is_platform: true,
            cursor_mode: cursor::EMBEDDED,
        }
    }
}

/// A live mutter ScreenCast session owning one virtual monitor.
pub struct ScreenCastSession {
    conn: Connection,
    session_path: OwnedObjectPath,
    stream_path: OwnedObjectPath,
    /// PipeWire node id from `PipeWireStreamAdded`.
    pub node_id: u32,
    pub config: SessionConfig,
    stopped: bool,
}

impl ScreenCastSession {
    /// CreateSession → RecordVirtual → wait PipeWireStreamAdded → Start.
    ///
    /// Blocks the calling thread (uses `pollster`). Call from a dedicated
    /// backend thread — not from the TCP accept loop.
    pub fn start(cfg: SessionConfig) -> Result<ScreenCastSession, GnomeError> {
        pollster::block_on(Self::start_async(cfg))
    }

    async fn start_async(cfg: SessionConfig) -> Result<ScreenCastSession, GnomeError> {
        let conn = Connection::session().await?;
        let root = ScreenCastProxy::new(&conn).await?;

        let empty: HashMap<&str, Value<'_>> = HashMap::new();
        let session_path = root.create_session(empty).await?;
        let session = SessionProxy::builder(&conn)
            .path(&session_path)?
            .build()
            .await?;

        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
        props.insert("cursor-mode", Value::U32(cfg.cursor_mode));
        if cfg.is_platform {
            props.insert("is-platform", Value::Bool(true));
        }
        let stream_path = session.record_virtual(props).await?;

        let stream = StreamProxy::builder(&conn)
            .path(&stream_path)?
            .build()
            .await?;
        let mut added = stream.receive_pipe_wire_stream_added().await?;

        session.start().await?;

        let node_id = match future::or(
            async {
                match added.next().await {
                    Some(signal) => match signal.args() {
                        Ok(a) => Some(Ok(a.node_id)),
                        Err(e) => Some(Err(GnomeError::from(e))),
                    },
                    None => Some(Err(GnomeError::Timeout("PipeWireStreamAdded closed"))),
                }
            },
            async {
                Timer::after(Duration::from_secs(8)).await;
                None
            },
        )
        .await
        {
            Some(Ok(id)) => id,
            Some(Err(e)) => return Err(e),
            None => return Err(GnomeError::Timeout("PipeWireStreamAdded")),
        };

        Ok(ScreenCastSession {
            conn,
            session_path,
            stream_path,
            node_id,
            config: cfg,
            stopped: false,
        })
    }

    pub fn session_path(&self) -> &str {
        self.session_path.as_str()
    }

    pub fn stream_path(&self) -> &str {
        self.stream_path.as_str()
    }

    /// Stop the session (destroys the virtual monitor). Idempotent.
    pub fn stop(&mut self) -> Result<(), GnomeError> {
        if self.stopped {
            return Ok(());
        }
        pollster::block_on(async {
            let session = SessionProxy::builder(&self.conn)
                .path(&self.session_path)?
                .build()
                .await?;
            session.stop().await?;
            Ok::<(), GnomeError>(())
        })?;
        self.stopped = true;
        Ok(())
    }
}

impl Drop for ScreenCastSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ---- generated proxies ----------------------------------------------------

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast"
)]
trait ScreenCast {
    fn create_session(
        &self,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast.Session",
    default_service = "org.gnome.Mutter.ScreenCast"
)]
trait Session {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn record_virtual(
        &self,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.gnome.Mutter.ScreenCast.Stream",
    default_service = "org.gnome.Mutter.ScreenCast"
)]
trait Stream {
    #[zbus(signal)]
    fn pipe_wire_stream_added(&self, node_id: u32) -> zbus::Result<()>;
}
