//! Command-line interface.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Serve clients over TCP.
    Serve,
    /// Run the frame-production benchmark; no network.
    SelfTest,
}

#[derive(Debug, Clone)]
pub struct Args {
    pub mode: Mode,
    pub source: String,
    pub bind: String,
    pub port: u16,
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    pub full_every_secs: u64,
    pub stats: Option<String>,
    pub frame_ack: bool,
    pub frames: u32,
}

pub const USAGE: &str = "\
usbra-host — USBra host (a real second display for Linux, over USB)

USAGE: usbra-host [OPTIONS]

  --source <name>        frame source: test (animated damage pattern).
                         'gnome' (mutter RecordVirtual) and 'evdi' land in M6.
  --bind <addr>          listen address (default 127.0.0.1 — loopback only)
  --port <n>             TCP port (default 8899; 0 = ephemeral)
  --width <n>            virtual display width in px (default 1600)
  --height <n>           virtual display height in px (default 900)
  --fps <n>              target fps (default 60)
  --full-frame-every <s> full-frame resync interval in seconds (default 2; 0=off)
  --stats <file>         write JSONL event log
  --frame-ack            ask client for FRAME_ACK (latency instrumentation)
  --selftest             benchmark frame production; no network, no phone
  --frames <n>           selftest frame count (default 600)
  -h, --help             this help

Typical demo run (with scripts/adb-usb-setup.sh active):
  usbra-host --source test --stats /tmp/usbra.jsonl --frame-ack
";

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        mode: Mode::Serve,
        source: "test".into(),
        bind: "127.0.0.1".into(),
        port: 8899,
        width: 1600,
        height: 900,
        fps: 60,
        full_every_secs: 2,
        stats: None,
        frame_ack: false,
        frames: 600,
    };
    let mut i = 0usize;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let val = |i: &mut usize, name: &str| -> Result<String, String> {
            *i += 1;
            argv.get(*i).cloned().ok_or_else(|| format!("{name} requires a value"))
        };
        match arg {
            "-h" | "--help" => return Err(String::new()),
            "--selftest" => a.mode = Mode::SelfTest,
            "--frame-ack" => a.frame_ack = true,
            "--source" => a.source = val(&mut i, arg)?,
            "--bind" => a.bind = val(&mut i, arg)?,
            "--port" => a.port = parse_num(&val(&mut i, arg)?, arg)?,
            "--width" => a.width = parse_num(&val(&mut i, arg)?, arg)?,
            "--height" => a.height = parse_num(&val(&mut i, arg)?, arg)?,
            "--fps" => a.fps = parse_num(&val(&mut i, arg)?, arg)?,
            "--full-frame-every" => a.full_every_secs = parse_num(&val(&mut i, arg)?, arg)?,
            "--stats" => a.stats = Some(val(&mut i, arg)?),
            "--frames" => a.frames = parse_num(&val(&mut i, arg)?, arg)?,
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    if a.source != "test" {
        return Err(format!(
            "source '{}' is not available yet — the gnome/evdi backends land in M6; \
             use --source test",
            a.source
        ));
    }
    if a.width == 0 || a.height == 0 || a.width > 8192 || a.height > 8192 {
        return Err("width/height must be within 1..=8192".into());
    }
    if a.fps == 0 || a.fps > 240 {
        return Err("fps must be within 1..=240".into());
    }
    if a.frames == 0 {
        a.frames = 1;
    }
    Ok(a)
}

fn parse_num<T: std::str::FromStr>(v: &str, name: &str) -> Result<T, String> {
    v.parse::<T>().map_err(|_| format!("{name}: invalid value {v:?}"))
}
