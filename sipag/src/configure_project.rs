use anyhow::{bail, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::templates;

struct TemplateFile {
    relative_path: &'static str,
    content: &'static str,
}

/// Claude Code hook templates installed to .claude/hooks/.
const CLAUDE_HOOK_TEMPLATES: &[TemplateFile] = &[TemplateFile {
    relative_path: "hooks/katulong-pubsub.sh",
    content: templates::HOOK_KATULONG_PUBSUB,
}];

/// All templates for the --static path (agents + commands only).
const TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        relative_path: "agents/security-reviewer.md",
        content: templates::AGENT_SECURITY_REVIEWER,
    },
    TemplateFile {
        relative_path: "agents/architecture-reviewer.md",
        content: templates::AGENT_ARCHITECTURE_REVIEWER,
    },
    TemplateFile {
        relative_path: "agents/correctness-reviewer.md",
        content: templates::AGENT_CORRECTNESS_REVIEWER,
    },
    TemplateFile {
        relative_path: "agents/backlog-triager.md",
        content: templates::AGENT_BACKLOG_TRIAGER,
    },
    TemplateFile {
        relative_path: "agents/issue-analyst.md",
        content: templates::AGENT_ISSUE_ANALYST,
    },
    TemplateFile {
        relative_path: "agents/root-cause-analyst.md",
        content: templates::AGENT_ROOT_CAUSE_ANALYST,
    },
    TemplateFile {
        relative_path: "agents/simplicity-advocate.md",
        content: templates::AGENT_SIMPLICITY_ADVOCATE,
    },
    TemplateFile {
        relative_path: "commands/dispatch.md",
        content: templates::COMMAND_DISPATCH,
    },
    TemplateFile {
        relative_path: "commands/review.md",
        content: templates::COMMAND_REVIEW,
    },
    TemplateFile {
        relative_path: "commands/triage.md",
        content: templates::COMMAND_TRIAGE,
    },
    TemplateFile {
        relative_path: "commands/ship-it.md",
        content: templates::COMMAND_SHIP_IT,
    },
    TemplateFile {
        relative_path: "commands/work.md",
        content: templates::COMMAND_WORK,
    },
    // commands/consult.md is installed separately by install_consult()
    // because the variant depends on whether `diwa` is available on PATH.
    TemplateFile {
        relative_path: "commands/release.md",
        content: templates::COMMAND_RELEASE,
    },
];

/// Git hook templates installed to .husky/ (separate from .claude/ templates).
const HOOK_TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        relative_path: "pre-commit",
        content: templates::GIT_HOOK_PRE_COMMIT,
    },
    TemplateFile {
        relative_path: "pre-push",
        content: templates::GIT_HOOK_PRE_PUSH,
    },
];

const CONFIGURE_PROMPT: &str = include_str!("../../lib/prompts/configure.md");

pub fn run_configure(dir: &Path, static_only: bool) -> Result<()> {
    let dir = if dir.is_relative() {
        std::env::current_dir()?.join(dir)
    } else {
        dir.to_path_buf()
    };
    let dir = dir.canonicalize().unwrap_or(dir);

    // Warn if not a git repository, but proceed.
    if !dir.join(".git").exists() {
        eprintln!(
            "warning: {} does not appear to be a git repository",
            dir.display()
        );
    }

    let claude_dir = dir.join(".claude");
    let husky_dir = dir.join(".husky");

    // Create directory structure.
    for subdir in &["agents", "commands", "hooks"] {
        fs::create_dir_all(claude_dir.join(subdir))?;
    }
    fs::create_dir_all(&husky_dir)?;

    if static_only || !claude_available() {
        if !static_only {
            eprintln!("claude CLI not found. Installing generic templates.");
            eprintln!(
                "Re-run sipag configure after installing Claude Code for project-specific setup.\n"
            );
        }
        return install_static_templates(&claude_dir, &husky_dir);
    }

    // Generative: launch Claude to explore the project and write
    // customized agents and commands.
    let prompt = build_configure_prompt();
    eprintln!("Launching Claude to set up agents and commands for this project...\n");
    exec_claude(&dir, &prompt)
}

fn claude_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// True if `diwa` is on PATH. Used to choose the diwa-aware variant of
/// the /consult command. The diwa-aware variant adds a Phase 0 that
/// mines git history before the review agents run.
fn diwa_available() -> bool {
    Command::new("diwa")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build the system prompt for the generative configure session.
/// Replaces placeholder tokens in the template with reference template content.
/// The `{COMMAND_CONSULT}` reference matches whichever variant would be
/// installed by the static path: diwa-aware if `diwa` is on PATH, otherwise
/// the base variant. This keeps the generative output consistent with what
/// `--static` would produce on the same machine.
pub(crate) fn build_configure_prompt() -> String {
    let consult_template = if diwa_available() {
        templates::COMMAND_CONSULT
    } else {
        templates::COMMAND_CONSULT_NO_DIWA
    };
    CONFIGURE_PROMPT
        .replace(
            "{AGENT_SECURITY_REVIEWER}",
            templates::AGENT_SECURITY_REVIEWER,
        )
        .replace(
            "{AGENT_ARCHITECTURE_REVIEWER}",
            templates::AGENT_ARCHITECTURE_REVIEWER,
        )
        .replace(
            "{AGENT_CORRECTNESS_REVIEWER}",
            templates::AGENT_CORRECTNESS_REVIEWER,
        )
        .replace("{AGENT_BACKLOG_TRIAGER}", templates::AGENT_BACKLOG_TRIAGER)
        .replace("{AGENT_ISSUE_ANALYST}", templates::AGENT_ISSUE_ANALYST)
        .replace(
            "{AGENT_ROOT_CAUSE_ANALYST}",
            templates::AGENT_ROOT_CAUSE_ANALYST,
        )
        .replace(
            "{AGENT_SIMPLICITY_ADVOCATE}",
            templates::AGENT_SIMPLICITY_ADVOCATE,
        )
        .replace("{COMMAND_DISPATCH}", templates::COMMAND_DISPATCH)
        .replace("{COMMAND_REVIEW}", templates::COMMAND_REVIEW)
        .replace("{COMMAND_TRIAGE}", templates::COMMAND_TRIAGE)
        .replace("{COMMAND_SHIP_IT}", templates::COMMAND_SHIP_IT)
        .replace("{COMMAND_WORK}", templates::COMMAND_WORK)
        .replace("{COMMAND_CONSULT}", consult_template)
        .replace("{COMMAND_RELEASE}", templates::COMMAND_RELEASE)
        .replace("{HOOK_PRE_COMMIT}", templates::GIT_HOOK_PRE_COMMIT)
        .replace("{HOOK_PRE_PUSH}", templates::GIT_HOOK_PRE_PUSH)
}

fn exec_claude(project_dir: &Path, prompt: &str) -> Result<()> {
    let context = discover_project(project_dir);
    let initial_message = format!(
        "Set up Claude Code for this project. Here is the project context \
         discovered by sipag — base all your work on this:\n\n{context}"
    );
    let err = Command::new("claude")
        .arg("--append-system-prompt")
        .arg(prompt)
        .arg(initial_message)
        .current_dir(project_dir)
        .exec();
    bail!("failed to exec claude: {err}")
}

/// Truncate a string at a UTF-8 safe boundary, appending "(truncated)" if needed.
fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...\n(truncated)", &s[..end])
}

/// Scan the project directory and return a structured context string.
/// This grounds the Claude session in actual project data rather than
/// relying on Claude to explore (and potentially hallucinate).
fn discover_project(dir: &Path) -> String {
    let mut sections = Vec::new();

    // 1. Top-level directory listing (skip hidden and empty).
    if let Ok(entries) = fs::read_dir(dir) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        if !names.is_empty() {
            sections.push(format!(
                "## Directory listing\n\n```\n{}\n```",
                names.join("\n")
            ));
        }
    }

    // 2. Config files — read whichever exist.
    let config_files = [
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "go.mod",
        "Makefile",
        "Gemfile",
        "composer.json",
        "pom.xml",
        "build.gradle",
        "deno.json",
        "bun.lockb",
    ];
    for name in &config_files {
        let path = dir.join(name);
        if let Ok(content) = fs::read_to_string(&path) {
            let truncated = truncate_utf8(&content, 2000);
            sections.push(format!("## {name}\n\n```\n{truncated}\n```"));
        }
    }

    // 3. README / CLAUDE.md.
    for name in &["README.md", "README", "CLAUDE.md"] {
        let path = dir.join(name);
        if let Ok(content) = fs::read_to_string(&path) {
            let truncated = truncate_utf8(&content, 3000);
            sections.push(format!("## {name}\n\n{truncated}"));
        }
    }

    if sections.is_empty() {
        "## Project context\n\nNo config files, README, or source directories found. \
         This appears to be an empty or minimal project."
            .to_string()
    } else {
        format!("## Project context\n\n{}", sections.join("\n\n"))
    }
}

fn install_static_templates(claude_dir: &Path, husky_dir: &Path) -> Result<()> {
    let installed = install_templates(claude_dir, TEMPLATES, ".claude/")?;
    install_consult(claude_dir)?;
    let claude_hooks_installed = install_claude_hooks(claude_dir)?;
    let settings_installed = install_settings(claude_dir)?;
    let hooks_installed = install_static_hooks(husky_dir)?;

    // Categorize for summary. consult.md is installed separately by
    // install_consult() but counts as a command.
    let agents = TEMPLATES
        .iter()
        .filter(|t| t.relative_path.starts_with("agents/"))
        .count();
    let commands = TEMPLATES
        .iter()
        .filter(|t| t.relative_path.starts_with("commands/"))
        .count()
        + 1;
    let total = installed + 1;

    println!("\nInstalled {total} files ({agents} agents, {commands} commands) to .claude/");
    println!("Installed {claude_hooks_installed} Claude Code hooks to .claude/hooks/");
    if settings_installed {
        println!("Installed settings.local.json to .claude/");
    }
    println!("Installed {hooks_installed} git hooks to .husky/");
    println!("\nActivate hooks:  git config core.hooksPath .husky");

    Ok(())
}

/// Install commands/consult.md, picking the diwa-aware variant if `diwa`
/// is on PATH and the base variant otherwise. The diwa-aware variant adds
/// a Phase 0 that mines git history (via the `diwa` skill) before the
/// review agents run, so they can avoid recommending approaches that
/// were already tried and reverted.
fn install_consult(claude_dir: &Path) -> Result<()> {
    let dest = claude_dir.join("commands/consult.md");
    let with_diwa = diwa_available();
    let content = if with_diwa {
        templates::COMMAND_CONSULT
    } else {
        templates::COMMAND_CONSULT_NO_DIWA
    };
    let action = if dest.exists() { "overwrite" } else { "create" };
    fs::write(&dest, content)?;
    let suffix = if with_diwa {
        " (diwa-aware variant — diwa detected on PATH)"
    } else {
        " (base variant — diwa not on PATH)"
    };
    println!("  {action}: .claude/commands/consult.md{suffix}");
    Ok(())
}

fn install_claude_hooks(claude_dir: &Path) -> Result<u32> {
    let installed = install_templates(claude_dir, CLAUDE_HOOK_TEMPLATES, ".claude/")?;
    for hook in CLAUDE_HOOK_TEMPLATES {
        set_executable(&claude_dir.join(hook.relative_path))?;
    }
    Ok(installed)
}

fn install_settings(claude_dir: &Path) -> Result<bool> {
    let dest = claude_dir.join("settings.local.json");
    let action = if dest.exists() { "overwrite" } else { "create" };
    fs::write(&dest, templates::SETTINGS_LOCAL)?;
    println!("  {action}: .claude/settings.local.json");
    Ok(true)
}

fn install_static_hooks(husky_dir: &Path) -> Result<u32> {
    let installed = install_templates(husky_dir, HOOK_TEMPLATES, ".husky/")?;
    for hook in HOOK_TEMPLATES {
        set_executable(&husky_dir.join(hook.relative_path))?;
    }
    Ok(installed)
}

fn set_executable(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)?;
    let mut perms = metadata.permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn install_templates(
    base_dir: &Path,
    templates: &[TemplateFile],
    display_prefix: &str,
) -> Result<u32> {
    let mut installed = 0u32;

    for template in templates {
        let dest = base_dir.join(template.relative_path);

        let action = if dest.exists() { "overwrite" } else { "create" };

        fs::write(&dest, template.content)?;
        println!("  {action}: {display_prefix}{}", template.relative_path);
        installed += 1;
    }

    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_configure_prompt_replaces_all_placeholders() {
        let prompt = build_configure_prompt();
        assert!(
            !prompt.contains("{AGENT_"),
            "prompt should not contain unreplaced {{AGENT_*}} placeholders"
        );
        assert!(
            !prompt.contains("{COMMAND_"),
            "prompt should not contain unreplaced {{COMMAND_*}} placeholders"
        );
        assert!(
            !prompt.contains("{HOOK_"),
            "prompt should not contain unreplaced {{HOOK_*}} placeholders"
        );
    }

    #[test]
    fn build_configure_prompt_contains_template_content() {
        let prompt = build_configure_prompt();
        // Should contain content from at least one reference template.
        assert!(prompt.contains("security"));
        assert!(prompt.contains("architecture"));
        assert!(prompt.contains("correctness"));
    }

    #[test]
    fn build_configure_prompt_contains_ship_it() {
        let prompt = build_configure_prompt();
        assert!(
            prompt.contains("ship-it"),
            "prompt should reference ship-it"
        );
    }

    #[test]
    fn build_configure_prompt_contains_boundary_constraints() {
        let prompt = build_configure_prompt();
        assert!(prompt.contains("Do NOT invent or hallucinate project details"));
        assert!(prompt.contains("Read ONLY files inside the current working directory"));
        assert!(prompt.contains("Do NOT explore parent directories"));
    }

    #[test]
    fn discover_project_reads_config_files() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name": "test-project", "version": "1.0.0"}"#,
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();

        let context = discover_project(dir.path());
        assert!(
            context.contains("test-project"),
            "should contain project name"
        );
        assert!(
            context.contains("package.json"),
            "should mention config file"
        );
        assert!(context.contains("src/"), "should list directories");
    }

    #[test]
    fn discover_project_empty_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let context = discover_project(dir.path());
        assert!(
            context.contains("empty or minimal"),
            "should note empty project"
        );
    }

    #[test]
    fn truncate_utf8_safe_on_multibyte() {
        // 'é' is 2 bytes in UTF-8. Place it at the boundary.
        let s = "a".repeat(1999) + "é" + "bbb";
        assert_eq!(s.len(), 2004); // 1999 + 2 + 3
        let result = truncate_utf8(&s, 2000);
        // Should NOT panic, and should truncate before the 'é'
        assert!(result.ends_with("(truncated)"));
        assert!(result.len() < 2020);
    }

    #[test]
    fn truncate_utf8_no_op_for_short_strings() {
        let s = "hello";
        assert_eq!(truncate_utf8(s, 2000), "hello");
    }

    #[test]
    fn discover_project_truncates_large_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let big = "x".repeat(3000);
        fs::write(dir.path().join("package.json"), &big).unwrap();

        let context = discover_project(dir.path());
        assert!(context.contains("(truncated)"));
        // The full 3000-char content should not be present.
        assert!(!context.contains(&big));
    }
}
