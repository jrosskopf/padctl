use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::Mutex;

struct FileLogger {
    writer: Mutex<BufWriter<File>>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(
                w,
                "[{level:<5} {target}] {args}",
                level = record.level(),
                target = record.target(),
                args = record.args(),
            );
            let _ = w.flush();
        }
    }

    fn flush(&self) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.flush();
        }
    }
}

/// Initialise file-based logging. Writes to `padctl-debug.log` in the current directory.
pub fn init() -> std::io::Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("padctl-debug.log")?;

    let logger = Box::leak(Box::new(FileLogger {
        writer: Mutex::new(BufWriter::new(file)),
    }));

    log::set_logger(logger)
        .map(|()| log::set_max_level(LevelFilter::Debug))
        .map_err(|e| std::io::Error::other(e.to_string()))
}
