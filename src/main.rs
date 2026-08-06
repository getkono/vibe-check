use eyre::Result;
use tracing::info;

fn status_message() -> &'static str {
    "vibe-check is ready"
}

async fn run() -> Result<()> {
    info!(message = status_message());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();
    run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_ready_status() {
        assert_eq!(status_message(), "vibe-check is ready");
    }
}
