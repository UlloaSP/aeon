//! A project's tasks: the scripts in `package.json`, the recipes in a
//! `justfile`, the targets in a `Makefile`, and cargo's own commands. The
//! parsers are pure; `detect` reads the files.

use std::path::Path;

/// Something a project can run, such as `dev` from `package.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    /// What actually runs: `npm run dev`, `cargo test`, `just build`.
    pub command: String,
    /// What the task runs, as the file states it: `vite`, `tsc -b`.
    pub detail: String,
    /// Where it came from: `package.json`, `justfile`, `Makefile`, `Cargo.toml`.
    pub source: &'static str,
}

/// More than any sidebar shows; a monorepo can list hundreds.
const LIMIT: usize = 40;

/// The tasks of the project at `root`.
pub fn detect(root: &Path) -> Vec<Task> {
    let read = |name: &str| std::fs::read_to_string(root.join(name)).ok();
    let mut tasks = Vec::new();
    if let Some(json) = read("package.json") {
        let runner = if root.join("pnpm-lock.yaml").exists() {
            "pnpm run"
        } else if root.join("yarn.lock").exists() {
            "yarn"
        } else if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
            "bun run"
        } else {
            "npm run"
        };
        tasks.extend(
            package_scripts(&json)
                .into_iter()
                .map(|(name, script)| Task {
                    command: format!("{runner} {name}"),
                    name,
                    detail: script,
                    source: "package.json",
                }),
        );
    }
    if let Some(text) = read("justfile").or_else(|| read("Justfile")) {
        tasks.extend(just_recipes(&text).into_iter().map(|name| Task {
            command: format!("just {name}"),
            detail: String::new(),
            name,
            source: "justfile",
        }));
    }
    if let Some(text) = read("Makefile").or_else(|| read("makefile")) {
        tasks.extend(make_targets(&text).into_iter().map(|name| Task {
            command: format!("make {name}"),
            detail: String::new(),
            name,
            source: "Makefile",
        }));
    }
    if root.join("Cargo.toml").exists() {
        for (name, detail) in [
            ("build", "cargo build"),
            ("test", "cargo test"),
            ("run", "cargo run"),
            ("clippy", "cargo clippy"),
        ] {
            tasks.push(Task {
                name: name.to_owned(),
                command: detail.to_owned(),
                detail: String::new(),
                source: "Cargo.toml",
            });
        }
    }
    tasks.truncate(LIMIT);
    tasks
}

/// The `scripts` of a `package.json`, in file order.
pub fn package_scripts(json: &str) -> Vec<(String, String)> {
    let mut parser = Json {
        chars: json.chars().collect(),
        at: 0,
    };
    let Some(Value::Object(top)) = parser.value() else {
        return Vec::new();
    };
    let Some((_, Value::Object(scripts))) = top.into_iter().find(|(k, _)| k == "scripts") else {
        return Vec::new();
    };
    scripts
        .into_iter()
        .filter_map(|(name, value)| match value {
            Value::String(script) => Some((name, script)),
            _ => None,
        })
        .collect()
}

/// Recipe names in a justfile: `name:` or `name arg:` at the start of a line.
pub fn just_recipes(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#']))
        .filter_map(|line| {
            let line = line.trim_start_matches('@');
            let (head, rest) = line.split_once(':')?;
            // `:=` is an assignment, not a recipe.
            if rest.starts_with('=') {
                return None;
            }
            let name = head.split_whitespace().next()?;
            let ok = name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
            (ok && !name.is_empty()).then(|| name.to_owned())
        })
        .collect()
}

/// Target names in a Makefile, without special and pattern targets.
pub fn make_targets(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || head.contains(['=', '%', '$']) {
            continue;
        }
        for name in head.split_whitespace() {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// Just enough JSON to read `package.json`.
enum Value {
    String(String),
    Object(Vec<(String, Value)>),
    Other,
}

struct Json {
    chars: Vec<char>,
    at: usize,
}

impl Json {
    fn skip(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip();
        if self.chars.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.skip();
        match self.chars.get(self.at)? {
            '{' => {
                self.at += 1;
                let mut fields = Vec::new();
                if self.eat('}') {
                    return Some(Value::Object(fields));
                }
                loop {
                    self.skip();
                    let key = self.string()?;
                    if !self.eat(':') {
                        return None;
                    }
                    fields.push((key, self.value()?));
                    if self.eat(',') {
                        continue;
                    }
                    return self.eat('}').then_some(Value::Object(fields));
                }
            }
            '[' => {
                self.at += 1;
                if self.eat(']') {
                    return Some(Value::Other);
                }
                loop {
                    self.value()?;
                    if self.eat(',') {
                        continue;
                    }
                    return self.eat(']').then_some(Value::Other);
                }
            }
            '"' => self.string().map(Value::String),
            _ => {
                // Numbers, true, false, null: skip to the next delimiter.
                while self
                    .chars
                    .get(self.at)
                    .is_some_and(|c| !matches!(c, ',' | '}' | ']') && !c.is_whitespace())
                {
                    self.at += 1;
                }
                Some(Value::Other)
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        if self.chars.get(self.at) != Some(&'"') {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            let c = *self.chars.get(self.at)?;
            self.at += 1;
            match c {
                '"' => return Some(out),
                '\\' => {
                    let e = *self.chars.get(self.at)?;
                    self.at += 1;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' | 'f' => {}
                        'u' => {
                            let hex: String =
                                self.chars.get(self.at..self.at + 4)?.iter().collect();
                            self.at += 4;
                            out.push(
                                char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or('?'),
                            );
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_scripts_keep_their_order() {
        let json = r#"{
            "name": "web-app", "version": "1.4.0", "private": true,
            "workspaces": ["a", {"b": 1}],
            "scripts": { "dev": "vite", "build": "tsc -b && vite build", "say": "echo \"hi\"" },
            "dependencies": { "vite": "^6.0.7" }
        }"#;
        assert_eq!(
            package_scripts(json),
            [
                ("dev".to_owned(), "vite".to_owned()),
                ("build".to_owned(), "tsc -b && vite build".to_owned()),
                ("say".to_owned(), "echo \"hi\"".to_owned()),
            ]
        );
        assert!(package_scripts("not json").is_empty());
        assert!(package_scripts(r#"{"name": "x"}"#).is_empty());
    }

    #[test]
    fn justfile_recipes_skip_assignments_and_bodies() {
        let text = "set shell := [\"pwsh\"]\nversion := \"1\"\n\nbuild:\n    cargo build\n\ntest filter='':\n    cargo test {{filter}}\n@fmt:\n    cargo fmt\n";
        assert_eq!(just_recipes(text), ["build", "test", "fmt"]);
    }

    #[test]
    fn make_targets_skip_special_and_pattern_rules() {
        let text = ".PHONY: all test\nCC := gcc\nall: build\nbuild test: deps\n\t$(CC) main.c\n%.o: %.c\n\tcc -c $<\n";
        assert_eq!(make_targets(text), ["all", "build", "test"]);
    }

    #[test]
    fn detect_reads_a_project_folder() {
        let dir = std::env::temp_dir().join(format!("aeon-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        let tasks = detect(&dir);
        assert_eq!(tasks[0].command, "pnpm run dev");
        assert_eq!(tasks[0].detail, "vite");
        assert!(tasks.iter().any(|t| t.command == "cargo test"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
