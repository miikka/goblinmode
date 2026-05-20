// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{fmt, prelude::*};

pub fn init(trace_arg: Option<&str>) -> Result<(Option<PathBuf>, Option<WorkerGuard>)> {
    let Some(arg) = trace_arg else {
        return Ok((None, None));
    };

    let path = if arg.is_empty() {
        default_trace_path()?
    } else {
        PathBuf::from(arg)
    };
    ensure_parent_dir(&path)?;

    let parent = path
        .parent()
        .context("Trace path must include a valid parent directory")?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Trace file name must be valid UTF-8")?;

    let appender = tracing_appender::rolling::never(parent, file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let layer = fmt::layer()
        .json()
        .with_span_events(FmtSpan::CLOSE)
        .with_writer(non_blocking);

    tracing_subscriber::registry().with(layer).init();

    Ok((Some(path), Some(guard)))
}

fn default_trace_path() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(cwd.join(format!("gob-trace-{}.jsonl", ts)))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("Trace path must include a parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "Failed to create parent directory for trace file: {}",
            parent.display()
        )
    })?;
    Ok(())
}
