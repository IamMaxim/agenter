mod browser_ws;
mod http;
mod runner_ws;
mod state;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    http::serve().await
}
