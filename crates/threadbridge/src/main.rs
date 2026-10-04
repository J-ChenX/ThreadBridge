mod adapter;
mod agent;
mod capture;
mod hub;
mod model;
mod notify;
#[cfg(unix)]
mod probe;
mod queue;
mod store;
#[cfg(test)]
mod tests;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use store::Store;

#[derive(Parser)]
#[command(
    name = "threadbridge",
    version,
    about = "续桥 · 手机接着聊，电脑接着做"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    CaptureSync {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        capture_db: PathBuf,
        #[arg(long)]
        host: String,
        #[arg(
            long,
            required_unless_present = "all_captured",
            conflicts_with = "all_captured"
        )]
        catalog: Option<PathBuf>,
        #[arg(long)]
        all_captured: bool,
    },
    CaptureBridge {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        capture_db: PathBuf,
        #[arg(long)]
        host: String,
        #[arg(long)]
        /// Limit to one conversation; omit to follow all captured conversations.
        thread: Option<String>,
        #[arg(long, default_value = "codex")]
        codex: PathBuf,
        #[arg(long)]
        allow_queue: bool,
        #[arg(long, requires = "allow_queue")]
        verified_version: Option<String>,
    },
    CaptureImport {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        capture_db: PathBuf,
        #[arg(long)]
        host: String,
        #[arg(long)]
        thread: String,
    },
    Doctor {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        thread: Option<String>,
    },
    Hub {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: String,
        #[arg(long, env = "THREADBRIDGE_NTFY_URL")]
        ntfy_url: Option<String>,
    },
    Pair {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long)]
        url: String,
    },
    RegisterAgent {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        output: PathBuf,
    },
    Agent {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "local/agent.sqlite")]
        db: PathBuf,
        #[arg(long, default_value = "codex")]
        codex: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        verified_version: Option<String>,
        #[arg(long)]
        demo: bool,
    },
    Combined {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: String,
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "local/agent.sqlite")]
        agent_db: PathBuf,
        #[arg(long, default_value = "codex")]
        codex: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        verified_version: Option<String>,
        #[arg(long)]
        demo: bool,
    },
    Revoke {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long)]
        device: String,
    },
    Backup {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Restore {
        #[arg(long)]
        backup: PathBuf,
        #[arg(long)]
        destination: PathBuf,
    },
    EnableWrites {
        #[arg(long, default_value = "local/hub.sqlite")]
        db: PathBuf,
        #[arg(long)]
        reconciled: bool,
    },
}
fn agent_config(
    path: PathBuf,
    db: PathBuf,
    codex: PathBuf,
    socket: Option<PathBuf>,
    verified_version: Option<String>,
    demo: bool,
) -> Result<agent::AgentConfig> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(agent::AgentConfig {
        hub: v["hub"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("config missing hub"))?
            .into(),
        token: v["token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("config missing token"))?
            .into(),
        host: v["host_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("config missing host_id"))?
            .into(),
        db,
        codex,
        socket,
        verified_version,
        demo,
        local: None,
    })
}
fn start_notifications(db: Store, url: String) -> Result<()> {
    notify::validate_url(&url)?;
    let token = std::env::var("THREADBRIDGE_NTFY_TOKEN").ok();
    tokio::spawn(async move {
        loop {
            if notify::run(db.clone(), url.clone(), token.clone())
                .await
                .is_err()
            {
                // Keep subscription details out of logs; durable outbox survives worker retry.
                eprintln!("Notification worker unavailable; retrying in 10 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
    Ok(())
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::CaptureSync {
            db,
            capture_db,
            host,
            catalog,
            all_captured,
        } => {
            anyhow::ensure!(db.is_file(), "existing Hub database required");
            capture::sync_catalog(Store::open(&db)?, capture_db, host, catalog, all_captured)
                .await?;
        }
        Commands::CaptureBridge {
            db,
            capture_db,
            host,
            thread,
            codex,
            allow_queue,
            verified_version,
        } => {
            anyhow::ensure!(db.is_file(), "existing Hub database required");
            capture::run(
                Store::open(&db)?,
                capture_db,
                host,
                thread,
                codex,
                allow_queue,
                verified_version,
            )
            .await?;
        }
        Commands::CaptureImport {
            db,
            capture_db,
            host,
            thread,
        } => {
            anyhow::ensure!(
                db.is_file(),
                "capture import requires an existing Hub database"
            );
            let shared = Store::open(&db)?;
            let count = capture::import(&shared, &capture_db, &host, &thread)?;
            println!("Imported {count} captured replies; target remains read-only");
        }
        Commands::Doctor { socket, thread } => {
            #[cfg(unix)]
            {
                let path = socket
                    .or_else(|| {
                        std::env::var_os("CODEX_HOME")
                            .map(PathBuf::from)
                            .or_else(|| {
                                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex"))
                            })
                            .map(|p| p.join("app-server-control/app-server-control.sock"))
                    })
                    .ok_or_else(|| anyhow::anyhow!("provide --socket"))?;
                match probe::probe(&path, thread.as_deref()).await {
                    Ok(v) => println!(
                        "{}",
                        serde_json::json!({"probe":"ok","g01":"not_verified","details":v})
                    ),
                    Err(e) => {
                        println!(
                            "{}",
                            serde_json::json!({"probe":"blocked","g01":"not_verified","error":e})
                        );
                        std::process::exit(2)
                    }
                }
            }
            #[cfg(not(unix))]
            anyhow::bail!("Unix probe unavailable on this platform");
        }
        Commands::Hub {
            db,
            listen,
            ntfy_url,
        } => {
            let db = Store::open(&db)?;
            db.recover()?;
            if let Some(url) = ntfy_url {
                start_notifications(db.clone(), url)?;
            }
            hub::run(db, &listen).await?;
        }
        Commands::Pair { db, url } => {
            anyhow::ensure!(
                url.starts_with("https://") || url.starts_with("http://"),
                "invalid server URL"
            );
            let code = Store::open(&db)?.pair_code()?;
            println!(
                "{}",
                serde_json::json!({"server":url,"code":code,"expires_in_seconds":300})
            );
        }
        Commands::RegisterAgent { db, name, output } => {
            anyhow::ensure!(!output.exists(), "refusing to overwrite credential file");
            let db = Store::open(&db)?;
            let count: i64 = db.0.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM devices WHERE role='agent' AND revoked=0",
                [],
                |r| r.get(0),
            )?;
            anyhow::ensure!(count < 4, "four active agents maximum");
            let (host_id, token) = db.credential("agent", &name)?;
            if let Some(p) = output.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut opt = std::fs::OpenOptions::new();
            opt.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opt.mode(0o600);
            }
            use std::io::Write;
            opt.open(&output)?.write_all(serde_json::to_string_pretty(&serde_json::json!({"host_id":host_id,"token":token,"hub":"http://127.0.0.1:8787"}))?.as_bytes())?;
            println!(
                "Created agent {host_id}; credential stored in {}",
                output.display()
            );
        }
        Commands::Agent {
            config,
            db,
            codex,
            socket,
            verified_version,
            demo,
        } => {
            agent::run(agent_config(
                config,
                db,
                codex,
                socket,
                verified_version,
                demo,
            )?)
            .await?
        }
        Commands::Combined {
            db,
            listen,
            config,
            agent_db,
            codex,
            socket,
            verified_version,
            demo,
        } => {
            let shared = Store::open(&db)?;
            shared.recover()?;
            let mut cfg = agent_config(config, agent_db, codex, socket, verified_version, demo)?;
            cfg.local = Some(shared.clone());
            if let Ok(url) = std::env::var("THREADBRIDGE_NTFY_URL") {
                start_notifications(shared.clone(), url)?;
            }
            tokio::spawn(async move {
                if let Err(e) = agent::run(cfg).await {
                    eprintln!("Agent stopped: {e}")
                }
            });
            hub::run(shared, &listen).await?;
        }
        Commands::Revoke { db, device } => Store::open(&db)?.revoke(&device)?,
        Commands::Backup { db, output } => {
            Store::open(&db)?.backup(&output)?;
            println!("Consistent SQLite backup: {}", output.display());
        }
        Commands::Restore {
            backup,
            destination,
        } => {
            anyhow::ensure!(
                !destination.exists(),
                "restore requires a new destination; stop service first"
            );
            let src = Store::open(&backup)?;
            src.backup(&destination)?;
            let db = Store::open(&destination)?;
            db.recover()?;
            db.0.lock().unwrap().execute_batch("UPDATE settings SET v='off' WHERE k='writes';UPDATE devices SET revoked=1;UPDATE commands SET status='unknown' WHERE status IN ('accepted','dispatching');UPDATE outbox SET sent=1;")?;
            println!("Restored read-only; re-pair devices, reconcile ledgers, then enable-writes --reconciled");
        }
        Commands::EnableWrites { db, reconciled } => {
            anyhow::ensure!(reconciled, "manual reconciliation confirmation required");
            Store::open(&db)?
                .0
                .lock()
                .unwrap()
                .execute("UPDATE settings SET v='on' WHERE k='writes'", [])?;
        }
    }
    Ok(())
}
