//! Diff model and a forge-agnostic unified diff parser.
//!
//! Accepts `git diff` output (with `diff --git` headers, renames, binary
//! markers and mode lines) as well as plain unified diffs. Hunk bodies are
//! delimited by the counts in their `@@` header, so content lines that look
//! like headers (`--- x`) are handled correctly.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub files: Vec<FileDiff>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// `None` for added files.
    pub old_path: Option<String>,
    /// `None` for deleted files.
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    fn empty() -> Self {
        Self {
            old_path: None,
            new_path: None,
            status: FileStatus::Modified,
            additions: 0,
            deletions: 0,
            binary: false,
            hunks: Vec::new(),
        }
    }

    /// The path to show: the new path, or the old one for deleted files.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// Text after the second `@@`, usually the enclosing function.
    pub section: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`, applying to the previous line.
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// The line without its `+`/`-`/` ` prefix.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid diff at line {line}: {message}")]
pub struct DiffParseError {
    pub line: usize,
    pub message: String,
}

impl Diff {
    pub fn parse_unified(input: &str) -> Result<Self, DiffParseError> {
        Parser::default().parse(input)
    }

    pub fn additions(&self) -> u32 {
        self.files.iter().map(|f| f.additions).sum()
    }

    pub fn deletions(&self) -> u32 {
        self.files.iter().map(|f| f.deletions).sum()
    }
}

struct OpenHunk {
    hunk: Hunk,
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
}

#[derive(Default)]
struct Parser {
    files: Vec<FileDiff>,
    file: Option<FileDiff>,
    hunk: Option<OpenHunk>,
}

impl Parser {
    fn parse(mut self, input: &str) -> Result<Diff, DiffParseError> {
        for (index, line) in input.lines().enumerate() {
            let line_no = index + 1;
            let err = |message: &str| DiffParseError {
                line: line_no,
                message: message.to_owned(),
            };
            if self.hunk.is_some() {
                self.hunk_line(line).map_err(err)?;
            } else {
                self.header_line(line).map_err(err)?;
            }
        }
        if self.hunk.is_some() {
            return Err(DiffParseError {
                line: input.lines().count(),
                message: "diff ends in the middle of a hunk".to_owned(),
            });
        }
        self.finish_file();
        Ok(Diff { files: self.files })
    }

    fn hunk_line(&mut self, line: &str) -> Result<(), &'static str> {
        let Some(open) = self.hunk.as_mut() else {
            return Ok(());
        };
        let (kind, text) = match line.chars().next() {
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some(' ') => (LineKind::Context, &line[1..]),
            Some('\\') => (LineKind::NoNewline, line),
            // Some tools strip the space from empty context lines.
            None => (LineKind::Context, ""),
            Some(_) => return Err("unexpected line inside a hunk"),
        };

        let (old_no, new_no) = match kind {
            LineKind::Context => {
                if open.old_left == 0 || open.new_left == 0 {
                    return Err("hunk has more lines than its header says");
                }
                open.old_left -= 1;
                open.new_left -= 1;
                open.old_no += 1;
                open.new_no += 1;
                (Some(open.old_no - 1), Some(open.new_no - 1))
            }
            LineKind::Removed => {
                if open.old_left == 0 {
                    return Err("hunk removes more lines than its header says");
                }
                open.old_left -= 1;
                open.old_no += 1;
                (Some(open.old_no - 1), None)
            }
            LineKind::Added => {
                if open.new_left == 0 {
                    return Err("hunk adds more lines than its header says");
                }
                open.new_left -= 1;
                open.new_no += 1;
                (None, Some(open.new_no - 1))
            }
            LineKind::NoNewline => (None, None),
        };
        open.hunk.lines.push(DiffLine {
            kind,
            old_no,
            new_no,
            text: text.to_owned(),
        });

        if open.old_left == 0 && open.new_left == 0 {
            self.close_hunk();
        }
        Ok(())
    }

    fn header_line(&mut self, line: &str) -> Result<(), &'static str> {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.finish_file();
            let mut file = FileDiff::empty();
            if let Some((old, new)) = split_git_paths(rest) {
                file.old_path = Some(old);
                file.new_path = Some(new);
            }
            self.file = Some(file);
        } else if let Some(path) = line.strip_prefix("--- ") {
            // A plain unified diff starts a new file at `---`.
            if self.file.as_ref().is_none_or(|f| !f.hunks.is_empty()) {
                self.finish_file();
                self.file = Some(FileDiff::empty());
            }
            let file = self.file_mut();
            match strip_side(path, "a/") {
                Some(path) => file.old_path = Some(path),
                None => {
                    file.old_path = None;
                    file.status = FileStatus::Added;
                }
            }
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let file = self.file_mut();
            match strip_side(path, "b/") {
                Some(path) => file.new_path = Some(path),
                None => {
                    file.new_path = None;
                    file.status = FileStatus::Deleted;
                }
            }
        } else if line.starts_with("new file mode") {
            let file = self.file_mut();
            file.status = FileStatus::Added;
            file.old_path = None;
        } else if line.starts_with("deleted file mode") {
            let file = self.file_mut();
            file.status = FileStatus::Deleted;
            file.new_path = None;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            let file = self.file_mut();
            file.status = FileStatus::Renamed;
            file.old_path = Some(path.to_owned());
        } else if let Some(path) = line.strip_prefix("rename to ") {
            let file = self.file_mut();
            file.status = FileStatus::Renamed;
            file.new_path = Some(path.to_owned());
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            self.file_mut().binary = true;
        } else if line.starts_with("@@") {
            if self.file.is_none() {
                return Err("hunk before any file header");
            }
            let header = parse_hunk_header(line).ok_or("malformed hunk header")?;
            if header.old_lines == 0 && header.new_lines == 0 {
                self.file_mut().hunks.push(header);
            } else {
                self.hunk = Some(OpenHunk {
                    old_left: header.old_lines,
                    new_left: header.new_lines,
                    old_no: header.old_start,
                    new_no: header.new_start,
                    hunk: header,
                });
            }
        } else if line.starts_with('\\') {
            // `\ No newline` right after a hunk that just closed.
            if let Some(hunk) = self.file.as_mut().and_then(|f| f.hunks.last_mut()) {
                hunk.lines.push(DiffLine {
                    kind: LineKind::NoNewline,
                    old_no: None,
                    new_no: None,
                    text: line.to_owned(),
                });
            }
        }
        // index, mode, similarity and other extended headers are ignored.
        Ok(())
    }

    fn file_mut(&mut self) -> &mut FileDiff {
        self.file.get_or_insert_with(FileDiff::empty)
    }

    fn close_hunk(&mut self) {
        if let Some(open) = self.hunk.take() {
            let file = self.file_mut();
            for line in &open.hunk.lines {
                match line.kind {
                    LineKind::Added => file.additions += 1,
                    LineKind::Removed => file.deletions += 1,
                    LineKind::Context | LineKind::NoNewline => {}
                }
            }
            file.hunks.push(open.hunk);
        }
    }

    fn finish_file(&mut self) {
        if let Some(mut file) = self.file.take() {
            if file.status == FileStatus::Modified
                && file.old_path.is_some()
                && file.old_path != file.new_path
            {
                file.status = FileStatus::Renamed;
            }
            self.files.push(file);
        }
    }
}

/// Splits `a/old b/new` from a `diff --git` line.
fn split_git_paths(rest: &str) -> Option<(String, String)> {
    let rest = rest.strip_prefix("a/")?;
    let (old, new) = rest.rsplit_once(" b/")?;
    Some((old.to_owned(), new.to_owned()))
}

/// `a/path` → `path`; `/dev/null` → `None`. Drops a trailing tab + timestamp.
fn strip_side(path: &str, prefix: &str) -> Option<String> {
    let path = path.split('\t').next().unwrap_or(path).trim_end();
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(prefix).unwrap_or(path).to_owned())
}

/// Parses `@@ -a[,b] +c[,d] @@ section`.
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let (old_start, old_lines) = parse_range(old.strip_prefix('-')?)?;
    let (new_start, new_lines) = parse_range(new.strip_prefix('+')?)?;
    Some(Hunk {
        section: section.trim().to_owned(),
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,3 @@ fn main() {
 fn main() {
-    println!(\"hi\");
+    println!(\"hello\");
 }
--- not a header, just a removed line
";

    #[test]
    fn parses_modified_file_with_line_numbers() {
        let diff = Diff::parse_unified(MODIFIED).unwrap();
        let [file] = diff.files.as_slice() else {
            panic!("expected one file");
        };
        assert_eq!(file.status, FileStatus::Modified);
        assert_eq!(file.path(), "src/lib.rs");
        assert_eq!((file.additions, file.deletions), (1, 2));
        let hunk = &file.hunks[0];
        assert_eq!(hunk.section, "fn main() {");
        let kinds: Vec<_> = hunk.lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(kinds, [Context, Removed, Added, Context, Removed]);
        assert_eq!(hunk.lines[1].old_no, Some(2));
        assert_eq!(hunk.lines[2].new_no, Some(2));
        assert_eq!(hunk.lines[4].text, "-- not a header, just a removed line");
    }

    #[test]
    fn parses_added_deleted_renamed_and_binary() {
        let input = "\
diff --git a/new.md b/new.md
new file mode 100644
--- /dev/null
+++ b/new.md
@@ -0,0 +1,2 @@
+# Title
+text
\\ No newline at end of file
diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/a.rs b/b.rs
similarity index 100%
rename from a.rs
rename to b.rs
diff --git a/logo.png b/logo.png
Binary files a/logo.png and b/logo.png differ
";
        let diff = Diff::parse_unified(input).unwrap();
        let statuses: Vec<_> = diff.files.iter().map(|f| f.status).collect();
        use FileStatus::*;
        assert_eq!(statuses, [Added, Deleted, Renamed, Modified]);

        let added = &diff.files[0];
        assert_eq!(added.old_path, None);
        assert_eq!(added.additions, 2);
        assert_eq!(
            added.hunks[0].lines.last().unwrap().kind,
            LineKind::NoNewline
        );

        let deleted = &diff.files[1];
        assert_eq!((deleted.path(), deleted.deletions), ("old.txt", 1));

        let renamed = &diff.files[2];
        assert_eq!(renamed.old_path.as_deref(), Some("a.rs"));
        assert_eq!(renamed.new_path.as_deref(), Some("b.rs"));
        assert!(renamed.hunks.is_empty());

        assert!(diff.files[3].binary);
        assert_eq!((diff.additions(), diff.deletions()), (2, 1));
    }

    #[test]
    fn parses_plain_unified_diff() {
        let input = "--- a.txt\t2026-01-01\n+++ a.txt\n@@ -1 +1 @@\n-x\n+y\n";
        let diff = Diff::parse_unified(input).unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path(), "a.txt");
    }

    #[test]
    fn empty_input_is_empty_diff() {
        assert_eq!(Diff::parse_unified("").unwrap(), Diff::default());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        for input in [
            "@@ -1 +1 @@\n-x\n+y\n",               // hunk before file
            "--- a\n+++ b\n@@ -x +1 @@\n",         // bad header
            "--- a\n+++ b\n@@ -1,2 +1,2 @@\n-x\n", // truncated
            "--- a\n+++ b\n@@ -1 +1 @@\n*weird\n", // bad line
        ] {
            assert!(Diff::parse_unified(input).is_err(), "{input:?}");
        }
    }
}
