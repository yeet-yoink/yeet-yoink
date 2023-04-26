use tracing::info;
use warp::Filter;

mod commands;
mod logging;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);
    info!("Hi. 👋");

    // GET /hello/warp => 200 OK with body "Hello, warp!"
    let hello = warp::path!("hello" / String).map(|name| format!("Hello, {}!", name));

    warp::serve(hello).run(([127, 0, 0, 1], 3030)).await;

    info!("Bye. 👋");
}
