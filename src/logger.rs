use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::{self, Write};

/// Stderr logger — writes to fd 2, which is separate from the TUI's stdout/alternate-screen.
///
/// Usage: run with `--debug` and redirect stderr:
///   padctl --debug 2>padctl-debug.log
struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let stderr = io::stderr();
        let mut h = stderr.lock();
        let _ = writeln!(
            h,
            "[{level:<5} {target}] {args}",
            level  = record.level(),
            target = record.target(),
            args   = record.args(),
        );
        let _ = h.flush();
    }

    fn flush(&self) {
        let _ = io::stderr().lock().flush();
    }
}

/// Initialise stderr logging.  Call before entering raw-mode / alternate screen.
/// Capture with:  padctl --debug 2>padctl-debug.log
pub fn init() -> std::io::Result<()> {
    log::set_logger(&StderrLogger)
        .map(|()| log::set_max_level(LevelFilter::Debug))
        .map_err(|e| std::io::Error::other(e.to_string()))
}
