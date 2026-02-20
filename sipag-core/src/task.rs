use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

/// A task parsed from a YAML-frontmatter task file.
#[derive(Debug, Default, Clone)]
pub struct Task {
    pub repo: String,
    pub priority: String,
    pub source: Option<String>,
    pub added: Option<String>,
    pub title: String,
    pub body: String,
}

/// Metadata parsed from a tracking file (written by the executor).
#[derive(Debug, Default)]
pub struct TrackingInfo {
    pub repo: String,
    pub issue: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub container: Option<String>,
    pub title: String,
}

/// Parse a task file with YAML frontmatter.
///
/// Format:
/// ```text
/// ---
/// repo: name
/// priority: medium
/// source: github#142
/// added: 2026-02-19T22:30:00Z
/// ---
/// Task Title
///
/// Optional body...
/// ```
pub fn parse_task_file(path: &Path) -> Result<Task> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    let lines: Vec<&str> = content.lines().collect();
    let n = lines.len();
    let mut i = 0;

    let mut task = Task {
        priority: "medium".to_string(),
        ..Default::default()
    };

    // Parse optional YAML frontmatter block
    if n > 0 && lines[0] == "---" {
        i = 1;
        while i < n {
            if lines[i] == "---" {
                i += 1;
                break;
            }
            if let Some(colon_pos) = lines[i].find(": ") {
                let key = lines[i][..colon_pos].trim();
                let value = lines[i][colon_pos + 2..].trim();
                match key {
                    "repo" => task.repo = value.to_string(),
                    "priority" => task.priority = value.to_string(),
                    "source" => task.source = Some(value.to_string()),
                    "added" => task.added = Some(value.to_string()),
                    _ => {}
                }
            }
            i += 1;
        }
    }

    // Find title: first non-empty line after frontmatter
    while i < n {
        if !lines[i].is_empty() {
            task.title = lines[i].to_string();
            i += 1;
            break;
        }
        i += 1;
    }

    // Skip leading blank lines before body
    while i < n && lines[i].is_empty() {
        i += 1;
    }

    // Find last non-empty line to trim trailing blanks
    let mut body_end = n;
    while body_end > i && lines[body_end - 1].is_empty() {
        body_end -= 1;
    }

    if i < body_end {
        task.body = lines[i..body_end].join("\n");
    }

    Ok(task)
}

/// Scan a file for `key: value` metadata lines (works across frontmatter and appended fields).
fn get_metadata_value(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for line in content.lines() {
        if line.starts_with(&prefix) {
            let value = line[prefix.len()..].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Parse a tracking file produced by the executor.
///
/// The file has YAML frontmatter with `repo`, `issue`, `started`, `container`,
/// and `ended` is appended after the closing `---` when the task completes.
pub fn parse_tracking_file(path: &Path) -> Result<TrackingInfo> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    let mut info = TrackingInfo {
        repo: get_metadata_value(&content, "repo").unwrap_or_default(),
        issue: get_metadata_value(&content, "issue"),
        started: get_metadata_value(&content, "started"),
        ended: get_metadata_value(&content, "ended"),
        container: get_metadata_value(&content, "container"),
        ..Default::default()
    };

    // Find title: first non-empty line after the closing --- of frontmatter
    let lines: Vec<&str> = content.lines().collect();
    let n = lines.len();
    let mut i = 0;

    if n > 0 && lines[0] == "---" {
        i = 1;
        while i < n {
            if lines[i] == "---" {
                i += 1;
                break;
            }
            i += 1;
        }
    }

    while i < n {
        if !lines[i].is_empty() && !lines[i].starts_with("ended:") {
            info.title = lines[i].to_string();
            break;
        }
        i += 1;
    }

    Ok(info)
}

/// Convert text to a URL-safe slug (lowercase, hyphens, no special chars).
///
/// Equivalent to: `tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-*//;s/-*$//'`
pub fn slugify(text: &str) -> String {
    let mut result = String::new();
    let mut last_was_hyphen = true; // start true to drop leading hyphens

    for c in text.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            result.push(lc);
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            result.push('-');
            last_was_hyphen = true;
        }
    }

    // Strip trailing hyphen
    while result.ends_with('-') {
        result.pop();
    }

    result
}

/// Generate the next sequential filename for a task (e.g. `003-fix-bug.md`).
pub fn next_filename(queue_dir: &Path, title: &str) -> Result<String> {
    let slug = slugify(title);
    let mut max_num: u32 = 0;

    if queue_dir.is_dir() {
        for entry in fs::read_dir(queue_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".md") {
                if let Some(num_str) = name.split('-').next() {
                    if let Ok(num) = num_str.parse::<u32>() {
                        max_num = max_num.max(num);
                    }
                }
            }
        }
    }

    Ok(format!("{:03}-{}.md", max_num + 1, slug))
}

/// Write a task file with YAML frontmatter.
pub fn write_task_file(
    path: &Path,
    title: &str,
    repo: &str,
    priority: &str,
    source: Option<&str>,
) -> Result<()> {
    let added = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut content = format!(
        "---\nrepo: {}\npriority: {}\n",
        repo, priority
    );
    if let Some(src) = source {
        content.push_str(&format!("source: {}\n", src));
    }
    content.push_str(&format!("added: {}\n---\n{}\n", added, title));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

/// Append a new pending checklist item to a markdown task file (legacy format).
///
/// Creates the file if it does not exist.
pub fn add_task(file: &Path, text: &str) -> Result<()> {
    let line = format!("- [ ] {}\n", text);
    if file.exists() {
        let mut f = fs::OpenOptions::new().append(true).open(file)?;
        f.write_all(line.as_bytes())?;
    } else {
        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(file, line)?;
    }
    Ok(())
}

/// Print all tasks from a markdown checklist file with done/pending counts.
pub fn list_tasks(file: &Path) -> Result<()> {
    if !file.exists() {
        println!("No task file: {}", file.display());
        return Ok(());
    }

    let content = fs::read_to_string(file)?;
    let mut done = 0usize;
    let mut pending = 0usize;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("- [x] ") {
            println!("  [x] {}", rest);
            done += 1;
        } else if let Some(rest) = line.strip_prefix("- [ ] ") {
            println!("  [ ] {}", rest);
            pending += 1;
        }
    }

    println!();
    println!("{}/{} done", done, done + pending);
    Ok(())
}

/// Mark the first unchecked task (`- [ ]`) as done (`- [x]`) at the given 1-based line number.
pub fn mark_done(file: &Path, line_number: usize) -> Result<()> {
    let content = fs::read_to_string(file)?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    if line_number == 0 || line_number > lines.len() {
        anyhow::bail!("line number {} out of range", line_number);
    }

    let idx = line_number - 1;
    lines[idx] = lines[idx].replacen("- [ ] ", "- [x] ", 1);

    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    fs::write(file, result)?;
    Ok(())
}

/// Find the first unchecked task in a markdown checklist.
///
/// Returns `(line_number, title, body)` where `line_number` is 1-based.
/// Returns `None` if no pending tasks are found.
pub fn parse_next(file: &Path) -> Result<Option<(usize, String, String)>> {
    if !file.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(file)?;
    let lines: Vec<&str> = content.lines().collect();
    let n = lines.len();
    let mut i = 0;
    let mut found_line = 0usize;
    let mut title = String::new();

    // Find the first unchecked item
    while i < n {
        if let Some(rest) = lines[i].strip_prefix("- [ ] ") {
            found_line = i + 1;
            title = rest.to_string();
            i += 1;
            break;
        }
        i += 1;
    }

    if found_line == 0 {
        return Ok(None);
    }

    // Collect indented body lines (2+ spaces)
    let mut body_lines: Vec<&str> = Vec::new();
    while i < n {
        if lines[i].starts_with("  ") {
            // Strip leading whitespace
            body_lines.push(lines[i].trim_start());
        } else {
            break;
        }
        i += 1;
    }

    let body = body_lines.join("\n");
    Ok(Some((found_line, title, body)))
}

/// Collect all `.md` files from a directory, sorted by filename.
pub fn sorted_md_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    files.sort();
    Ok(files)
}
