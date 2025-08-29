use clap::{Parser, Subcommand};

mod server;
mod client;
type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the shelld server
    Serve,
    /// Attach to a running shelld server
    Attach(client::ClientCli),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve => server::serve().await,
        Commands::Attach(client_cli) => client::attach(client_cli).await,
    }
    
}
