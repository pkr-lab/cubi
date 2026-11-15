use crate::runner::Status;

const TYPES: &[&str] = &[
    "feat", "fix", "docs", "chore", "refactor", "test", "ci", "build", "perf", "style", "revert",
];

#[derive(Debug, PartialEq, Eq)]
pub struct Conventional {
    pub kind: String,
    pub breaking: bool,
    pub desc: String,
}

pub fn parse_conventional(subject: &str) -> Option<Conventional> {
    let (head, desc) = subject.split_once(": ")?;
    let breaking = head.ends_with('!');
    let head = head.trim_end_matches('!');
    let kind = match head.split_once('(') {
        Some((k, scope)) if scope.ends_with(')') => k,
        Some(_) => return None,
        None => head,
    };
    if !TYPES.contains(&kind) || desc.trim().is_empty() {
        return None;
    }
    Some(Conventional {
        kind: kind.to_string(),
        breaking,
        desc: desc.trim().to_string(),
    })
}

fn branch_kind(branch: &str) -> Option<&str> {
    let prefix = branch.split('/').next()?;
    let prefix = match prefix {
        "feature" => "feat",
        "bugfix" | "hotfix" => "fix",
        p => p,
    };
    TYPES.iter().copied().find(|t| *t == prefix)
}

fn branch_words(branch: &str) -> String {
    let slug = branch.split_once('/').map_or(branch, |(_, s)| s);
    slug.replace(['-', '_', '/'], " ").trim().to_string()
}

pub fn pr_title(project: &str, branch: &str, subjects: &[String]) -> String {
    let parsed: Vec<Conventional> = subjects
        .iter()
        .filter_map(|s| parse_conventional(s))
        .collect();
    let breaking = parsed.iter().any(|c| c.breaking);
    let (kind, desc) = if subjects.len() == 1 && parsed.len() == 1 {
        (parsed[0].kind.clone(), parsed[0].desc.clone())
    } else {
        let kind = branch_kind(branch)
            .map(str::to_string)
            .or_else(|| {
                ["feat", "fix"]
                    .iter()
                    .find(|k| parsed.iter().any(|c| c.kind == **k))
                    .map(|k| k.to_string())
            })
            .or_else(|| parsed.first().map(|c| c.kind.clone()))
            .unwrap_or_else(|| "chore".into());
        let words = branch_words(branch);
        let desc = if words.is_empty() {
            parsed
                .first()
                .map(|c| c.desc.clone())
                .unwrap_or_else(|| "Änderungen".into())
        } else {
            words
        };
        (kind, desc)
    };
    let bang = if breaking { "!" } else { "" };
    let mut title = format!("{kind}({project}){bang}: {desc}");
    while title.ends_with('.') {
        title.pop();
    }
    title.chars().take(120).collect()
}

pub fn pr_body(subjects: &[String], checks: &[(String, bool, Status)]) -> String {
    let mut b = String::from("## Änderungen\n\n");
    if subjects.is_empty() {
        b.push_str("- (keine Commits gegenüber dem Basis-Branch)\n");
    }
    for s in subjects {
        b.push_str(&format!("- {s}\n"));
    }
    b.push_str("\n## Lokale Checks (cubi-precheck)\n\n");
    for (name, required, status) in checks {
        let icon = match status {
            Status::Ok => "✅",
            Status::Skipped(_) => "⏭️",
            Status::Failed(_) | Status::Cancelled => "❌",
            Status::Missing(_) => "⚠️",
            Status::Pending | Status::Running => "⏳",
        };
        let opt = if *required { "" } else { " (optional)" };
        let detail = status
            .detail()
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        b.push_str(&format!(
            "- {icon} {name}{opt}, {}{detail}\n",
            status.label()
        ));
    }
    b
}

pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for ch in text.to_lowercase().chars() {
        match ch {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            _ => {
                if !out.ends_with('-') {
                    out.push('-');
                }
            }
        }
    }
    let s: String = out.trim_matches('-').chars().take(40).collect();
    s.trim_end_matches('-').to_string()
}

pub fn branch_suggestion(subjects: &[String]) -> String {
    match subjects.last() {
        Some(s) => match parse_conventional(s) {
            Some(c) => format!("{}/{}", c.kind, slugify(&c.desc)),
            None => format!("feat/{}", slugify(s)),
        },
        None => "feat/neue-aenderung".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_conventional() {
        let c = parse_conventional("feat(core)!: neue Karte").unwrap();
        assert_eq!(c.kind, "feat");
        assert!(c.breaking);
        assert_eq!(c.desc, "neue Karte");
        assert!(parse_conventional("Update README").is_none());
        assert!(parse_conventional("foo: bar").is_none());
    }

    #[test]
    fn title_from_single_commit_uses_project_scope() {
        let t = pr_title(
            "capulus-core",
            "feat/x",
            &v(&["fix(ci): Zeitlimit erhöht."]),
        );
        assert_eq!(t, "fix(capulus-core): Zeitlimit erhöht");
    }

    #[test]
    fn title_from_branch_for_multiple_commits() {
        let t = pr_title(
            "capulus-ios",
            "feat/controlplane-1-statusanzeige",
            &v(&["fix: a", "feat: b"]),
        );
        assert_eq!(t, "feat(capulus-ios): controlplane 1 statusanzeige");
        let t = pr_title("x", "wip", &v(&["docs: a", "fix: b"]));
        assert_eq!(t, "fix(x): wip");
        let t = pr_title("x", "feat/y", &v(&["feat!: a", "fix: b"]));
        assert_eq!(t, "feat(x)!: y");
    }

    #[test]
    fn body_lists_commits_and_checks() {
        let b = pr_body(
            &v(&["feat: a"]),
            &[
                ("lint".into(), true, Status::Ok),
                (
                    "codeql".into(),
                    false,
                    Status::Missing("codeql fehlt".into()),
                ),
            ],
        );
        assert!(b.contains("- feat: a"));
        assert!(b.contains("✅ lint, ok"));
        assert!(b.contains("⚠️ codeql (optional), Voraussetzung fehlt: codeql fehlt"));
    }

    #[test]
    fn branch_suggestions() {
        assert_eq!(
            branch_suggestion(&v(&["fix(ci): Größere Zeitlimits für Läufe"])),
            "fix/groessere-zeitlimits-fuer-laeufe"
        );
        assert_eq!(branch_suggestion(&[]), "feat/neue-aenderung");
        assert_eq!(slugify("--A  b--"), "a-b");
    }
}
