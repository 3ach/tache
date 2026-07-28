mod dag;
mod server;
mod sync;
mod todoist;

use anyhow::{Result, bail};
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
    let client = Client::from_env()?;

    match cli.command {
        Command::Serve { bind } => {
            let secret = std::env::var("TODOIST_CLIENT_SECRET").unwrap_or_default();
            server::serve(&bind, client, secret).await?;
        }
        Command::Sync => {
            let report = sync::reconcile(&client).await?;
            println!(
                "{} tasks: {} next, {} blocked, {} relabeled",
                report.total, report.next, report.blocked, report.relabeled
            );
            if !report.unresolved.is_empty() || !report.ambiguous.is_empty() || report.cycles > 0 {
                println!("run `tache doctor` — graph has warnings");
            }
        }
        Command::Frontier => {
            let tasks = client.active_tasks().await?;
            let dag = Dag::build(&tasks);
            print!("{}", sync::format_frontier(&tasks, &dag.classify(&tasks)));
        }
        Command::Graph => {
            let tasks = client.active_tasks().await?;
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
            let tasks = client.active_tasks().await?;
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
            let tasks = client.active_tasks().await?;
            let target = find_unique(&tasks, &task)?;
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
                let p = find_unique(&tasks, &prereq)?;
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
