//! usbra-host — USBra host process.

use std::process::ExitCode;

use usbra_host::args::{self, Mode};
use usbra_host::{selftest, server};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match args::parse(&argv) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            print!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}\n\n{}", args::USAGE.trim_end());
            return ExitCode::from(2);
        }
    };

    eprintln!(
        "usbra-host v{} — source={} {}x{}@{}fps full-every={}s frame_ack={}",
        env!("CARGO_PKG_VERSION"),
        a.source,
        a.width,
        a.height,
        a.fps,
        a.full_every_secs,
        a.frame_ack
    );

    match a.mode {
        Mode::SelfTest => {
            selftest::run(&a);
            ExitCode::SUCCESS
        }
        Mode::Serve => {
            let cfg = server::ServeConfig {
                bind: a.bind.clone(),
                port: a.port,
                width: a.width,
                height: a.height,
                fps: a.fps,
                full_every_secs: a.full_every_secs,
                stats_path: a.stats.clone(),
                frame_ack: a.frame_ack,
            };
            match server::serve(&cfg) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(1)
                }
            }
        }
    }
}
