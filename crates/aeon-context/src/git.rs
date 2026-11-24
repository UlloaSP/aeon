use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Git {
    /// Branch name, or the short commit when HEAD is detached.
    pub head: String,
    /// Changed, staged, conflicted, and untracked paths.
    pub changed: u32,
    pub ahead: u32,
    pub behind: u32,
}

/// The repository `dir` belongs to: the nearest ancestor holding `.git`, a
/// folder in a repository or a file in a worktree or submodule.
pub fn root(dir: &Path) -> Option<std::path::PathBuf> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `None` outside a repository, or when git is not installed.
pub fn status(dir: &Path) -> Option<Git> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Aeon is a GUI app: without this, every poll flashes a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| parse(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses `git status --porcelain=v2 --branch`.
fn parse(porcelain: &str) -> Git {
    let mut git = Git::default();
    let mut oid = "";
    for line in porcelain.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => oid = value,
                "branch.head" => git.head = value.to_owned(),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            git.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            git.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
        } else if !line.is_empty() {
            git.changed += 1;
        }
    }
    if git.head == "(detached)" {
        git.head = oid.chars().take(7).collect();
    }
    git
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_the_nearest_folder_with_git() {
        let base = std::env::temp_dir().join(format!("aeon-root-{}", std::process::id()));
        let deep = base.join("repo").join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(base.join("repo").join(".git")).unwrap();
        assert_eq!(root(&deep), Some(base.join("repo")));
        // A worktree or submodule has a `.git` file instead.
        std::fs::write(base.join("repo").join("src").join(".git"), "gitdir: x").unwrap();
        assert_eq!(root(&deep), Some(base.join("repo").join("src")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parses_branch_counts_and_changes() {
        let git = parse(
            "# branch.oid 0df995a8caa07187449118165b9d3562575e74b0\n\
             # branch.head feature/bar\n\
             # branch.upstream origin/feature/bar\n\
             # branch.ab +2 -1\n\
             1 .M N... 100644 100644 100644 aaa bbb src/a.rs\n\
             ? notes.txt\n",
        );
        assert_eq!(
            git,
            Git {
                head: "feature/bar".into(),
                changed: 2,
                ahead: 2,
                behind: 1,
            }
        );
    }

    #[test]
    fn detached_head_shows_short_commit() {
        let git = parse(
            "# branch.oid 0df995a8caa07187449118165b9d3562575e74b0\n# branch.head (detached)\n",
        );
        assert_eq!(git.head, "0df995a");
        assert_eq!((git.changed, git.ahead, git.behind), (0, 0, 0));
    }

    #[test]
    fn this_repository_has_a_head() {
        let git = status(Path::new(env!("CARGO_MANIFEST_DIR")));
        if let Some(git) = git {
            assert!(!git.head.is_empty());
        }
    }

    #[test]
    fn outside_a_repository_is_none() {
        assert_eq!(status(Path::new(r"C:\")), None);
    }
}
