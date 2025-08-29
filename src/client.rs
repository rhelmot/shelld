use clap::Args;

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
use thiserror::Error;

/// Start a client to attach to an existing shelld server.
#[derive(Args)]
pub struct ClientCli {
    /// The HTTP address of the shelld server to attach to.
    #[arg(short, long, default_value = "http://localhost:3000")]
    address: String,
}

pub async fn attach(options: ClientCli) -> Result<(), Error> {
    println!("attaching to {}", options.address);
    Ok(())
}
