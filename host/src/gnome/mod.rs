//! GNOME / mutter `org.gnome.Mutter.ScreenCast` backend (M6).
//!
//! Creates a real virtual monitor via `RecordVirtual(is-platform=true)` and
//! consumes frames through `scripts/m6-pw-grab.py` (GStreamer pipewiresrc).
//! Native pipewire-rs + `SPA_META_VideoDamage` replaces the grabber once
//! `libpipewire-0.3-dev` is available.
//!
//! Compile with `--features gnome` (pulls in `zbus`).

mod dbus;
pub mod probe;
pub mod source;

pub use dbus::{cursor, GnomeError, ScreenCastSession, SessionConfig};
pub use source::GnomeSource;
