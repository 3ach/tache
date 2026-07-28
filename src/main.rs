mod dag;
mod server;
mod sync;
mod todoist;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use dag::Dag;
use todoist::Client;

#[derive(Parser)]
#[command(name = "tache", about = "Task DAG layer over Todoist")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the webhook server (the normal deployment mode)
    Serve {
        #[arg(long, env = "TACHE_BIND", default_value = "127.0.0.1:8321")]
        bind: String,
    },
    /// One-shot reconcile: recompute the frontier and fix labels
    Sync,
    /// List currently actionable tasks
    Frontier,
    /// Print the dependency graph
    Graph,
    /// Report unresolved / ambiguous references and cycles
    Doctor,
    /// Add a dependency: TASK will be blocked until PREREQ is done
    Dep {
        task: String,
        prereq: String,
        /// Remove the dependency instead of adding it
        #[arg(long)]
        rm: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tache=info".into()),
        )
        .init();

    let cli = Cli::parse();

    // Serve mode can start without a token (OAuth provides one); the CLI
    // commands need one up front.
    if let Command::Serve { bind } = &cli.command {
        let client = std::sync::Arc::new(Client::new(resolve_token().unwrap_or_default()));
        let oauth = server::Oauth {
            client_id: std::env::var("TODOIST_CLIENT_ID").unwrap_or_default(),
            client_secret: std::env::var("TODOIST_CLIENT_SECRET").unwrap_or_default(),
            token_file: token_file(),
        };
        return server::serve(bind, client, oauth).await;
    }

    let client = Client::new(resolve_token().context(
        "no API token: set TODOIST_API_TOKEN or complete OAuth so the token file exists",
    )?);

    match cli.command {
        Command::Serve { .. } => unreachable!("handled above"),
        Command::Sync => {
            let report = sync::reconcile(&client).await?;
            println!(
                "{} tasks in {} #dag projects: {} next, {} blocked, {} relabeled, {} stripped",
                report.scoped,
                report.projects,
                report.next,
                report.blocked,
                report.relabeled,
                report.stripped
            );
            if !report.unresolved.is_empty() || !report.ambiguous.is_empty() || report.cycles > 0 {
                println!("run `tache doctor` — graph has warnings");
            }
        }
        Command::Frontier => {
            let tasks = sync::Scope::fetch(&client).await?.enabled_tasks();
            let dag = Dag::build(&tasks);
            print!("{}", sync::format_frontier(&tasks, &dag.classify(&tasks)));
        }
        Command::Graph => {
            let tasks = sync::Scope::fetch(&client).await?.enabled_tasks();
            let dag = Dag::build(&tasks);
            let name = |id: &str| {
                tasks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.content.clone())
                    .unwrap_or_else(|| id.to_string())
            };
            for (task, prereqs) in &dag.prereqs {
                for p in prereqs {
                    println!("{}  <-  {}", name(task), name(p));
                }
            }
        }
        Command::Doctor => {
            let tasks = sync::Scope::fetch(&client).await?.enabled_tasks();
            let dag = Dag::build(&tasks);
            let name = |id: &str| {
                tasks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.content.clone())
                    .unwrap_or_else(|| id.to_string())
            };
            for (id, r) in &dag.unresolved {
                println!("unresolved  {}: after: {r}  (completed prereq or typo)", name(id));
            }
            for (id, r) in &dag.ambiguous {
                println!("ambiguous   {}: after: {r}  (matches multiple tasks)", name(id));
            }
            for id in dag.cycle_members() {
                println!("cycle       {}", name(&id));
            }
        }
        Command::Dep { task, prereq, rm } => {
            let scope = sync::Scope::fetch(&client).await?;
            let tasks = &scope.tasks;
            let target = find_unique(tasks, &task)?;
            if !scope.enabled_projects.contains(&target.project_id) {
                println!(
                    "note: this project is not #dag-enabled — add '{}' to its description",
                    sync::DAG_MARKER
                );
            }
            if rm {
                let needle = prereq.to_lowercase();
                let new_desc: String = target
                    .description
                    .lines()
                    .filter(|l| {
                        !(l.trim().to_lowercase().starts_with("after:")
                            && l.to_lowercase().contains(&needle))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                client.set_description(&target.id, &new_desc).await?;
                println!("removed: {} after {}", target.content, prereq);
            } else {
                // verify the prereq resolves before writing it
                let p = find_unique(tasks, &prereq)?;
                if p.project_id != target.project_id {
                    bail!("'{}' and '{}' are in different projects", target.content, p.content);
                }
                let mut desc = target.description.clone();
                if !desc.is_empty() {
                    desc.push('\n');
                }
                desc.push_str(&format!("after: {}", p.content));
                client.set_description(&target.id, &desc).await?;
                println!("added: {} after {}", target.content, p.content);
            }
        }
    }
    Ok(())
}

fn token_file() -> std::path::PathBuf {
    std::env::var("TACHE_TOKEN_FILE")
        .unwrap_or_else(|_| "tache-token".into())
        .into()
}

/// A token minted via /oauth/callback outlives whatever is in env.
fn resolve_token() -> Option<String> {
    std::fs::read_to_string(token_file())
        .map(|t| t.trim().to_string())
        .ok()
        .filter(|t| !t.is_empty())
        .or_else(|| std::env::var("TODOIST_API_TOKEN").ok().filter(|t| !t.is_empty()))
}

fn find_unique<'a>(tasks: &'a [todoist::Task], query: &str) -> Result<&'a todoist::Task> {
    if let Some(t) = tasks.iter().find(|t| t.id == query) {
        return Ok(t);
    }
    let needle = query.to_lowercase();
    let matches: Vec<&todoist::Task> = tasks
        .iter()
        .filter(|t| t.content.to_lowercase().contains(&needle))
        .collect();
    match matches.as_slice() {
        [t] => Ok(t),
        [] => bail!("no active task matches '{query}'"),
        many => bail!(
            "'{query}' is ambiguous, matches: {}",
            many.iter()
                .map(|t| t.content.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        ),
    }
}
