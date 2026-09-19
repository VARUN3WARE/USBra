//! Frame sources: anything that can produce sequenced `Frame`s with damage.
//!
//! M0–M5: [`crate::testsource::TestSource`].
//! M6: `GnomeScreenCast` (mutter `RecordVirtual` + PipeWire) plugs in here.

use usbra_protocol::Frame;

/// Producer of display frames for the protocol server.
pub trait FrameSource: Send {
    /// Virtual display size in pixels.
    fn size(&self) -> (u16, u16);

    /// Human-readable backend name for logs (`test`, `gnome`, …).
    fn name(&self) -> &'static str;

    /// Produce the next frame. `timestamp_ns` is a host wall-clock hint
    /// ([`usbra_protocol::now_ns`]); sources that pace themselves may ignore it
    /// and stamp their own capture time.
    fn next_frame(&mut self, timestamp_ns: u64) -> Frame;

    /// When true, the producer thread should not sleep between frames — the
    /// source blocks until the next capture (PipeWire / GStreamer).
    fn paces_itself(&self) -> bool {
        false
    }

    /// Tear down compositor resources (ScreenCast session, etc.). Default: no-op.
    fn shutdown(&mut self) {}
}
