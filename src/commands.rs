use crate::logging::LoggingStyle;
use clap::{Arg, Command};

pub fn build_command() -> Command {
    let command = Command::new("Yeet/Yoink")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Markus Mayer")
        .about("A service for storing and retrieving files")
        .arg(
            Arg::new("logging_style")
                .long("log")
                .env("APP_LOG_STYLE")
                .value_name("STYLE")
                .default_value("simple")
                .help("The logging style to use (simple, json)")
                .num_args(1)
                .value_parser(logging_style)
                .help_heading("Logging"),
        );
    command
}

fn logging_style(s: &str) -> Result<LoggingStyle, String> {
    match s {
        "simple" => Ok(LoggingStyle::Compact),
        "compact" => Ok(LoggingStyle::Compact),
        "json" => Ok(LoggingStyle::Json),
        _ => Err(String::from("Either simple or json must be specified")),
    }
}
