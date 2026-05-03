mod api;
mod auth;
mod browser_ws;
mod policy;
mod runner_ws;
mod state;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agenter_core::logging::init_tracing("agenter-control-plane");
    api::serve().await
}
