use crate::logging::LoggingStyle;
use clap::{Arg, Command};
use std::net::SocketAddr;
use std::str::FromStr;

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
        )
        .arg(
            Arg::new("bind_http")
                .long("http")
                .env("APP_SERVER_BIND_HTTP")
                .value_name("SOCKET")
                .default_value("127.0.0.1:8080")
                .help("The socket to bind insecure HTTP on")
                .num_args(1..)
                .allow_negative_numbers(false)
                .action(clap::ArgAction::Append)
                .value_parser(socket_addr)
                .help_heading("Server"),
        )
        .arg(
            Arg::new("dial")
                .long("dial")
                .value_name("MULTIADDR")
                .help("The multiaddr to dial")
                .num_args(1)
                .allow_negative_numbers(false)
                .help_heading("libp2p"),
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

fn socket_addr(s: &str) -> Result<SocketAddr, String> {
    SocketAddr::from_str(s).map_err(|e| format!("{e}"))
}
