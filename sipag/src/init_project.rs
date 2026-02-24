use anyhow::Result;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::templates;

struct TemplateFile {
    relative_path: &'static str,
    content: &'static str,
    executable: bool,
}

const TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        relative_path: "agents/security-reviewer.md",
        content: templates::AGENT_SECURITY_REVIEWER,
        executable: false,
    },
    TemplateFile {
        relative_path: "agents/architecture-reviewer.md",
        content: templates::AGENT_ARCHITECTURE_REVIEWER,
        executable: false,
    },
    TemplateFile {
        relative_path: "agents/correctness-reviewer.md",
        content: templates::AGENT_CORRECTNESS_REVIEWER,
        executable: false,
    },
    TemplateFile {
        relative_path: "agents/backlog-triager.md",
        content: templates::AGENT_BACKLOG_TRIAGER,
        executable: false,
    },
    TemplateFile {
        relative_path: "agents/issue-analyst.md",
        content: templates::AGENT_ISSUE_ANALYST,
        executable: false,
    },
    TemplateFile {
        relative_path: "commands/dispatch.md",
        content: templates::COMMAND_DISPATCH,
        executable: false,
    },
    TemplateFile {
        relative_path: "commands/review.md",
        content: templates::COMMAND_REVIEW,
        executable: false,
    },
    TemplateFile {
        relative_path: "hooks/safety-gate.sh",
        content: templates::HOOK_SAFETY_GATE_SH,
        executable: true,
    },
    TemplateFile {
        relative_path: "hooks/safety-gate.toml",
        content: templates::HOOK_SAFETY_GATE_TOML,
        executable: false,
    },
    TemplateFile {
        relative_path: "hooks/README.md",
        content: templates::HOOK_README,
        executable: false,
    },
    TemplateFile {
        relative_path: "settings.local.json",
        content: templates::SETTINGS_LOCAL_JSON,
        executable: false,
    },
];

pub fn run_init(dir: &Path, force: bool) -> Result<()> {
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

    // Create directories.
    for subdir in &["agents", "commands", "hooks"] {
        fs::create_dir_all(claude_dir.join(subdir))?;
    }

    let mut installed = 0u32;
    let mut skipped = 0u32;

    for template in TEMPLATES {
        let dest = claude_dir.join(template.relative_path);

        if dest.exists() && !force {
            println!(
                "  skip: .claude/{} (already exists)",
                template.relative_path
            );
            skipped += 1;
            continue;
        }

        fs::write(&dest, template.content)?;

        if template.executable {
            let mut perms = fs::metadata(&dest)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dest, perms)?;
        }

        let action = if force && dest.exists() {
            "overwrite"
        } else {
            "create"
        };
        println!("  {action}: .claude/{}", template.relative_path);
        installed += 1;
    }

    // Categorize for summary.
    let agents = TEMPLATES
        .iter()
        .filter(|t| t.relative_path.starts_with("agents/"))
        .count();
    let commands = TEMPLATES
        .iter()
        .filter(|t| t.relative_path.starts_with("commands/"))
        .count();
    let hooks = TEMPLATES
        .iter()
        .filter(|t| t.relative_path.starts_with("hooks/"))
        .count();

    println!(
        "\nInstalled {installed} files ({agents} agents, {commands} commands, {hooks} hooks + settings) to .claude/"
    );
    if skipped > 0 {
        println!("Skipped {skipped} existing files (use --force to overwrite)");
    }

    Ok(())
}
