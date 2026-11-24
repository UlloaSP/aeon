//! Links in terminal text: URLs, and file paths with an optional line and
//! column such as `src/cart.test.ts:42:17`. Pure: whether a path exists is
//! checked only when it is opened.

/// What a link points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Url(String),
    File {
        path: String,
        line: Option<u32>,
        col: Option<u32>,
    },
}

/// A link found in a line, covering columns `start..=end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub target: Target,
}

/// The link covering column `col` of `line`, if there is one. `line` holds
/// one character per column.
pub fn link_at(line: &[char], col: usize) -> Option<Link> {
    if col >= line.len() || line[col].is_whitespace() {
        return None;
    }
    let bounded = |c: char| c.is_whitespace() || "\"'<>`|()[]{}".contains(c);
    let mut start = col;
    while start > 0 && !bounded(line[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < line.len() && !bounded(line[end + 1]) {
        end += 1;
    }
    // Sentence punctuation after a link is not part of it.
    while end > start && ".,;!?".contains(line[end]) {
        end -= 1;
    }
    if col > end {
        return None;
    }
    let word: String = line[start..=end].iter().collect();
    let target = if word.starts_with("http://") || word.starts_with("https://") {
        Target::Url(word)
    } else {
        file_target(&word)?
    };
    Some(Link { start, end, target })
}

fn file_target(word: &str) -> Option<Target> {
    // Up to two trailing `:number`s are the line and column; a drive's colon
    // is never followed by a number.
    let mut path = word;
    let mut numbers = Vec::new();
    while numbers.len() < 2 {
        match path.rsplit_once(':') {
            Some((head, tail)) if tail.parse::<u32>().is_ok_and(|n| n > 0) => {
                numbers.push(tail.parse::<u32>().unwrap_or(0));
                path = head;
            }
            _ => break,
        }
    }
    numbers.reverse();
    looks_like_path(path).then(|| Target::File {
        path: path.to_owned(),
        line: numbers.first().copied(),
        col: numbers.get(1).copied(),
    })
}

/// A separator and a file name with an extension, or a drive path: enough to
/// tell `src/app.rs` from an ordinary word without touching the disk.
fn looks_like_path(path: &str) -> bool {
    let has_separator = path.contains('\\') || path.contains('/');
    let name = path.rsplit(['\\', '/']).next().unwrap_or("");
    let has_extension = name
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 8);
    let drive = path.as_bytes().get(1) == Some(&b':')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    has_separator && (has_extension || drive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, col: usize) -> Option<Link> {
        let chars: Vec<char> = text.chars().collect();
        link_at(&chars, col)
    }

    #[test]
    fn urls_stop_at_quotes_and_trailing_punctuation() {
        let link = at("see https://example.com/a?b=1. ok", 8).unwrap();
        assert_eq!(link.target, Target::Url("https://example.com/a?b=1".into()));
        assert_eq!((link.start, link.end), (4, 28));
        let link = at("(http://localhost:5173/)", 5).unwrap();
        assert_eq!(link.target, Target::Url("http://localhost:5173/".into()));
    }

    #[test]
    fn paths_carry_their_line_and_column() {
        let file = |path: &str, line, col| Target::File {
            path: path.into(),
            line,
            col,
        };
        assert_eq!(
            at(" ❯ src/cart.test.ts:42:17", 5).unwrap().target,
            file("src/cart.test.ts", Some(42), Some(17))
        );
        assert_eq!(
            at("crates/aeon/src/app.rs:120", 3).unwrap().target,
            file("crates/aeon/src/app.rs", Some(120), None)
        );
        assert_eq!(
            at(r"C:\code\web-app\package.json", 4).unwrap().target,
            file(r"C:\code\web-app\package.json", None, None)
        );
        assert_eq!(
            at(r"C:\code\x.rs:7:3", 2).unwrap().target,
            file(r"C:\code\x.rs", Some(7), Some(3))
        );
    }

    #[test]
    fn ordinary_words_are_not_links() {
        assert_eq!(at("hello world", 2), None);
        assert_eq!(at("version 1.2.3", 9), None);
        assert_eq!(at("a b", 1), None);
    }
}
