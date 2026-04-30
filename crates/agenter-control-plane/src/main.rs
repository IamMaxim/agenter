mod auth;
mod browser_ws;
mod http;
mod runner_ws;
mod state;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agenter_core::logging::init_tracing("agenter-control-plane");
    http::serve().await
}
