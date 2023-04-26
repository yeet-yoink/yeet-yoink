use clap::ArgMatches;
use std::borrow::Borrow;
use tracing::metadata::LevelFilter;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum LoggingStyle {
    /// Uses compact logging.
    Compact,
    /// Uses JSON formatted logging
    Json,
}

/// Initializes the tracing and logging system from arguments.
///
/// This method uses the default environment filter to configure logging.
/// Please use the `RUST_LOG` environment variable to tune.
///
/// ## Arguments
/// * `matches` - The clap argument matches.
pub fn initialize_from_matches<M: Borrow<ArgMatches>>(matches: M) {
    let style: &LoggingStyle = matches.borrow().get_one("logging_style").unwrap();
    initialize(style)
}

/// Initializes the tracing and logging system.
///
/// This method uses the default environment filter to configure logging.
/// Please use the `RUST_LOG` environment variable to tune.
///
/// ## Arguments
/// * `style` - The logging style to use.
pub fn initialize<S: Borrow<LoggingStyle>>(style: S) {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    let formatter = tracing_subscriber::fmt()
        .with_file(false)
        .with_line_number(false)
        .with_thread_ids(true)
        .with_target(true)
        .with_env_filter(filter);

    match style.borrow() {
        LoggingStyle::Compact => formatter.init(),
        LoggingStyle::Json => formatter.json().init(),
    }
}
