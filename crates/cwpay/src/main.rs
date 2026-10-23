#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pku_cwpay::run().await
}
