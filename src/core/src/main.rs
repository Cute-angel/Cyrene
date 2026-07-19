use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use clap::{Parser, Subcommand};
use cyrene_core::{
    Application, CoreConfig, OpenAiCompatibleEmbeddingProvider, Storage,
    embedding::EmbeddingProvider,
    singleton::{InstanceGuard, InstanceLock},
    transport::{run_http, run_stdio},
};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "cyrene-core", version, about = "Cyrene procedural memory core")]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize the data directory and issue the first administrator token.
    Init {
        #[arg(long, default_value = "owner")]
        admin_name: String,
    },
    /// Run the authenticated local HTTP transport.
    Serve {
        #[arg(long)]
        bind: Option<SocketAddr>,
    },
    /// Run the authenticated JSON Lines transport on stdin/stdout.
    Stdio,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(path) => path,
        None => CoreConfig::default_data_dir()?,
    };
    match cli.command {
        Command::Init { admin_name } => {
            let config = CoreConfig::initialize(&data_dir)?;
            let storage = Storage::new(&config.database);
            storage.migrate()?;
            let issued = storage.bootstrap_admin(&admin_name)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "initialized": issued.is_some(),
                    "data_dir": data_dir,
                    "admin": issued,
                    "warning": issued.as_ref().map(|_| "Store this token securely; it will not be shown again."),
                }))?
            );
        }
        Command::Serve { bind } => {
            let _instance = match InstanceGuard::acquire(&data_dir)? {
                InstanceLock::Acquired(guard) => guard,
                InstanceLock::AlreadyRunning => {
                    println!(
                        "{}",
                        serde_json::to_string(&json!({
                            "status": "already_running",
                            "data_dir": data_dir,
                        }))?
                    );
                    return Ok(());
                }
            };
            let config = CoreConfig::load(&data_dir)?.with_bind(bind)?;
            let app = build_application(&config)?;
            run_http(app, config.http_bind).await?;
        }
        Command::Stdio => {
            let config = CoreConfig::load(&data_dir)?;
            let app = build_application(&config)?;
            let token = std::env::var("CYRENE_ACCESS_TOKEN")
                .map_err(|_| anyhow::anyhow!("CYRENE_ACCESS_TOKEN is required for stdio mode"))?;
            run_stdio(app, &token).await?;
        }
    }
    Ok(())
}

fn build_application(config: &CoreConfig) -> anyhow::Result<Application> {
    let storage = Storage::new(&config.database);
    storage.migrate()?;
    let embedding: Option<Arc<dyn EmbeddingProvider>> = if config.embedding.enabled {
        OpenAiCompatibleEmbeddingProvider::from_environment(
            config.embedding.base_url.clone(),
            &config.embedding.api_key_env,
            config.embedding.model.clone(),
            config.embedding.dimensions,
            Duration::from_secs(config.embedding.request_timeout_seconds),
        )?
        .map(|provider| Arc::new(provider) as Arc<dyn EmbeddingProvider>)
    } else {
        None
    };
    Ok(Application::new(
        storage,
        embedding,
        config.embedding.model.clone(),
        config.embedding.dimensions,
    ))
}
