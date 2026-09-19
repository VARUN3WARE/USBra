//! GNOME / mutter `org.gnome.Mutter.ScreenCast` backend (M6).
//!
//! Creates a real virtual monitor via `RecordVirtual(is-platform=true)` and
//! exposes the resulting PipeWire node id. Frame extraction (PipeWire stream
//! + `SPA_META_VideoDamage`) lands next (needs `libpipewire-0.3-dev`); this
//! module is the D-Bus half so Display 2 can be created/torn down from Rust.
//!
//! Compile with `--features gnome` (pulls in `zbus`).

mod dbus;
pub mod probe;

pub use dbus::{cursor, GnomeError, ScreenCastSession, SessionConfig};
