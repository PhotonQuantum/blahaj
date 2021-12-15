use std::str;
use std::sync::atomic::{AtomicUsize, Ordering};

use ansi_term::Style;
use flexi_logger::{style, AdaptiveFormat, DeferredNow};
use log::{Level, Record};
use time::format_description::well_known::Rfc3339;
use time::format_description::FormatItem;
use time::macros::format_description;

static MAX_TARGET_WIDTH: AtomicUsize = AtomicUsize::new(0);

pub const PALETTE: &str = "9;3;10;12;5";
pub const FORMAT_FUNCTION: AdaptiveFormat =
    AdaptiveFormat::Custom(default_format, colored_default_format);

const TS_RFC3389_MS: &[FormatItem<'static>] = format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:2][offset_hour sign:mandatory]:[offset_minute]"
);

fn max_target_width(target: &str) -> usize {
    let max_width = MAX_TARGET_WIDTH.load(Ordering::Relaxed);
    if max_width < target.len() {
        MAX_TARGET_WIDTH.store(target.len(), Ordering::Relaxed);
        target.len()
    } else {
        max_width
    }
}

fn pad(s: &str, width: usize) -> String {
    format!("{: <width$}", s, width = width)
}

const fn map_level(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN ",
        Level::Info => "INFO ",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

pub fn colored_default_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let level = record.level();
    let target = record.target();
    let max_width = max_target_width(target);
    write!(
        w,
        " {} {} {} > {}",
        now.format(&TS_RFC3389_MS),
        style(level).bold().paint(map_level(level)),
        Style::default().bold().paint(pad(target, max_width)),
        record.args().to_string(),
    )
}

pub fn default_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    now.format(&TS_RFC3389_MS);
    let target = record.target();
    let max_width = max_target_width(target);
    write!(
        w,
        " {} {} {} > {}",
        now.format(&Rfc3339),
        map_level(record.level()),
        pad(target, max_width),
        record.args(),
    )
}
