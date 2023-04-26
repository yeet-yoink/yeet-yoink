use tracing::info;

mod commands;
mod logging;

fn main() {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);
    info!("Hi. 👋");

    info!("Bye. 👋");
}
