use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Result, bail};
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct RepoState {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub head: String,
    pub remote_slug: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<String>,
}

impl RepoState {
    pub fn fingerprint(&self) -> String {
        format!("{}|{}", self.head, self.changes.join("\n"))
    }
}

pub async fn run(repo: &Path, prog: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(prog)
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        bail!(
            "{prog} {} fehlgeschlagen: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    run(repo, "git", args).await
}

pub async fn state(path: &Path) -> Result<RepoState> {
    let root = PathBuf::from(git(path, &["rev-parse", "--show-toplevel"]).await?.trim());
    let head = git(&root, &["rev-parse", "HEAD"]).await?.trim().to_string();
    let status = git(&root, &["status", "--porcelain=v1", "--branch"]).await?;
    let mut lines = status.lines();
    let header = parse_branch_header(lines.next().unwrap_or_default());
    let remote = git(&root, &["remote", "get-url", "origin"]).await.ok();
    Ok(RepoState {
        root,
        branch: header.branch,
        head,
        remote_slug: remote.as_deref().and_then(parse_remote_slug),
        upstream: header.upstream,
        ahead: header.ahead,
        behind: header.behind,
        changes: lines.map(str::to_string).collect(),
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BranchHeader {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

pub fn parse_branch_header(line: &str) -> BranchHeader {
    let mut h = BranchHeader::default();
    let Some(rest) = line.strip_prefix("## ") else {
        return h;
    };
    let (names, counts) = match rest.find(" [") {
        Some(i) => (&rest[..i], &rest[i + 2..rest.len().saturating_sub(1)]),
        None => (rest, ""),
    };
    if names.starts_with("HEAD (no branch)") {
        return h;
    }
    let names = names.strip_prefix("No commits yet on ").unwrap_or(names);
    match names.split_once("...") {
        Some((b, u)) => {
            h.branch = Some(b.to_string());
            h.upstream = Some(u.to_string());
        }
        None => h.branch = Some(names.to_string()),
    }
    for part in counts.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            h.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            h.behind = n.parse().unwrap_or(0);
        }
    }
    h
}

pub fn parse_remote_slug(url: &str) -> Option<String> {
    let url = url.trim();
    let path = if let Some(p) = url.strip_prefix("git@github.com:") {
        p
    } else {
        let p = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("ssh://git@"))?;
        p.strip_prefix("github.com/")?
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let mut it = path.split('/');
    let (owner, repo) = (it.next()?, it.next()?);
    if it.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

pub async fn fetch_base(repo: &Path, base_branch: &str) -> Result<()> {
    git(repo, &["fetch", "--quiet", "origin", "--", base_branch]).await?;
    Ok(())
}

pub async fn merge_base(repo: &Path, base_branch: &str) -> Result<String> {
    let remote_ref = format!("refs/remotes/origin/{base_branch}");
    Ok(git(repo, &["merge-base", "HEAD", &remote_ref])
        .await?
        .trim()
        .to_string())
}

pub async fn changed_files(repo: &Path, base: &str) -> Result<Vec<String>> {
    let range = format!("{base}..HEAD");
    let mut files: Vec<String> = Vec::new();
    for args in [
        vec!["diff", "--name-only", range.as_str()],
        vec!["diff", "--name-only", "HEAD"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        files.extend(git(repo, &args).await?.lines().map(str::to_string));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

pub async fn commit_subjects(repo: &Path, base: &str) -> Result<Vec<String>> {
    let range = format!("{base}..HEAD");
    Ok(git(
        repo,
        &["log", "--reverse", "--no-merges", "--format=%s", &range],
    )
    .await?
    .lines()
    .map(str::to_string)
    .collect())
}

pub fn push_allowed(branch: Option<&str>, protected: &[String]) -> Result<(), String> {
    match branch {
        None => Err("Kein Branch ausgecheckt (detached HEAD)".into()),
        Some(b) if protected.iter().any(|p| p == b) => Err(format!(
            "Push auf geschützten Branch {b} ist gesperrt. Bitte einen neuen Branch anlegen."
        )),
        Some(_) => Ok(()),
    }
}

pub async fn valid_branch_name(repo: &Path, name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && git(repo, &["check-ref-format", "--branch", name])
            .await
            .is_ok()
}

pub async fn create_branch(repo: &Path, name: &str, protected: &[String]) -> Result<String> {
    if protected.iter().any(|p| p == name) {
        bail!("{name} ist geschützt");
    }
    if !valid_branch_name(repo, name).await {
        bail!("Ungültiger Branch-Name: {name}");
    }
    git(repo, &["switch", "-c", name]).await
}

pub async fn push(repo: &Path, branch: &str, protected: &[String]) -> Result<String> {
    push_allowed(Some(branch), protected).map_err(anyhow::Error::msg)?;
    if !valid_branch_name(repo, branch).await {
        bail!("Ungültiger Branch-Name: {branch}");
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let out = Command::new("git")
        .args(["push", "--set-upstream", "origin", &refspec])
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .await?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        bail!("git push fehlgeschlagen:\n{text}");
    }
    Ok(text)
}

pub async fn pr_create(
    repo: &Path,
    base: &str,
    head: &str,
    title: &str,
    body: &str,
    protected: &[String],
) -> Result<String> {
    push_allowed(Some(head), protected).map_err(anyhow::Error::msg)?;
    if title.trim().is_empty() {
        bail!("Titel fehlt");
    }
    run(
        repo,
        "gh",
        &[
            "pr", "create", "--base", base, "--head", head, "--title", title, "--body", body,
        ],
    )
    .await
}

pub async fn existing_pr(repo: &Path, branch: &str) -> Option<String> {
    run(
        repo,
        "gh",
        &["pr", "view", branch, "--json", "url", "--jq", ".url"],
    )
    .await
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

pub async fn gh_auth_status() -> Result<()> {
    let ok = Command::new("gh")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?
        .success();
    if !ok {
        bail!("gh ist nicht angemeldet (gh auth login)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_header_variants() {
        let h = parse_branch_header("## feat/x...origin/feat/x [ahead 2, behind 1]");
        assert_eq!(h.branch.as_deref(), Some("feat/x"));
        assert_eq!(h.upstream.as_deref(), Some("origin/feat/x"));
        assert_eq!((h.ahead, h.behind), (2, 1));
        let h = parse_branch_header("## main");
        assert_eq!(h.branch.as_deref(), Some("main"));
        assert_eq!(h.upstream, None);
        let h = parse_branch_header("## HEAD (no branch)");
        assert_eq!(h.branch, None);
        let h = parse_branch_header("## No commits yet on main");
        assert_eq!(h.branch.as_deref(), Some("main"));
    }

    #[test]
    fn remote_slugs() {
        for url in [
            "git@github.com:pkr-lab/cubi.git",
            "https://github.com/pkr-lab/cubi",
            "https://github.com/pkr-lab/cubi.git",
            "ssh://git@github.com/pkr-lab/cubi.git",
        ] {
            assert_eq!(
                parse_remote_slug(url).as_deref(),
                Some("pkr-lab/cubi"),
                "{url}"
            );
        }
        assert_eq!(parse_remote_slug("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn push_blocked_on_protected() {
        let p = vec!["main".to_string()];
        assert!(push_allowed(Some("main"), &p).is_err());
        assert!(push_allowed(None, &p).is_err());
        assert!(push_allowed(Some("feat/x"), &p).is_ok());
    }

    #[tokio::test]
    async fn real_repo_state_and_branch_ops() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.invalid"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "feat: start"],
        ] {
            git(r, &args).await.unwrap();
        }
        std::fs::write(r.join("neu.txt"), "x").unwrap();
        let s = state(r).await.unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.changes, vec!["?? neu.txt"]);
        let protected = vec!["main".to_string()];
        assert!(create_branch(r, "main", &protected).await.is_err());
        assert!(create_branch(r, "-x", &protected).await.is_err());
        assert!(create_branch(r, "a..b", &protected).await.is_err());
        create_branch(r, "feat/neu", &protected).await.unwrap();
        assert_eq!(state(r).await.unwrap().branch.as_deref(), Some("feat/neu"));
        assert!(push(r, "main", &protected).await.is_err());
        let base = git(r, &["rev-parse", "HEAD"]).await.unwrap();
        let files = changed_files(r, base.trim()).await.unwrap();
        assert_eq!(files, vec!["neu.txt"]);
    }
}
