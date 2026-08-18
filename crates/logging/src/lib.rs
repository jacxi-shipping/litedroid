use std::path::Path;

use tracing::info;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::daily;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use litedroid_core::{LiteDroidError, Result};

/// Initialise the tracing subsystem with a coloured console layer and a
/// JSON, daily-rotating file layer (7 day retention).
///
/// Returns a [`WorkerGuard`] that **must** be held for the lifetime of the
/// application so that the non-blocking writer can flush on shutdown.
pub fn init(level: &str, log_dir: &Path) -> Result<WorkerGuard> {
    let file_appender = daily(log_dir, "litedroid.log");
    let (writer, guard) = NonBlocking::new(file_appender);

    let filter =
        EnvFilter::try_new(level).map_err(|e| LiteDroidError::ConfigError(e.to_string()))?;

    let console_layer = fmt::layer().with_target(true).with_ansi(true);

    let file_layer = fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .json()
        .with_writer(writer);

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    info!(level = level, log_dir = %log_dir.display(), "logging initialised");
    Ok(guard)
}

/// Log a message indicating the logging subsystem is shutting down.
///
/// The actual flush happens when the [`WorkerGuard`] returned by [`init`]
/// is dropped.
pub fn shutdown() {
    info!("logging subsystem shutting down");
}
