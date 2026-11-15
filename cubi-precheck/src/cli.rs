use std::path::Path;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::{Check, Config};
use crate::git;
use crate::runner::{self, Event, Gate, Status};

pub fn run(cfg: &Config, repo: &Path) -> Result<bool> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let state = git::state(repo).await?;
        let slug = state
            .remote_slug
            .clone()
            .context("kein GitHub-Remote 'origin'")?;
        let profile = cfg
            .profile_for_remote(&slug)
            .with_context(|| format!("kein Profil für {slug}"))?
            .clone();
        let (warning, ctx) = runner::prepare(&state.root, &profile.base_branch, &cfg.dir).await?;
        if let Some(w) = warning {
            eprintln!("{w}");
        }
        println!(
            "{slug} | Branch {} | Basis {} | {} geänderte Dateien",
            state.branch.as_deref().unwrap_or("(detached)"),
            &ctx.base[..ctx.base.len().min(10)],
            ctx.changed.len()
        );
        let checks: Vec<Check> = profile.checks.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                c2.cancel();
            }
        });
        let list = checks.iter().cloned().enumerate().collect();
        let runner = tokio::spawn(runner::run_all(list, ctx, cfg.app.parallel, tx, cancel));
        let mut statuses = vec![Status::Pending; checks.len()];
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::Log { idx, line } => println!("[{}] {line}", checks[idx].id),
                Event::Status {
                    idx,
                    status,
                    elapsed,
                } => {
                    if status.is_final() {
                        println!(
                            "[{}] => {}{}{}",
                            checks[idx].id,
                            status.label(),
                            status
                                .detail()
                                .map(|d| format!(": {d}"))
                                .unwrap_or_default(),
                            elapsed
                                .map(|e| format!(" ({:.1}s)", e.as_secs_f32()))
                                .unwrap_or_default()
                        );
                    }
                    statuses[idx] = status;
                }
                Event::AllDone => break,
            }
        }
        runner.await?;
        println!("\nZusammenfassung:");
        for (c, s) in checks.iter().zip(&statuses) {
            println!(
                "  {:<22} {:<20} {}{}",
                c.id,
                s.label(),
                if c.required { "" } else { "(optional) " },
                s.detail().unwrap_or_default()
            );
        }
        Ok(match runner::gate(&checks, &statuses) {
            Gate::Green => {
                println!("\nAlle Pflicht-Checks grün.");
                true
            }
            Gate::Blocked(b) => {
                println!("\nGesperrt: {}", b.join("; "));
                false
            }
        })
    })
}
