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
        relative_path: "commands/triage.md",
        content: templates::COMMAND_TRIAGE,
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

const INIT_PROMPT: &str = include_str!("../../lib/prompts/init.md");

pub fn run_init(dir: &Path, force: bool, static_only: bool) -> Result<()> {
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

    // Create directory structure.
    for subdir in &["agents", "commands", "hooks"] {
        fs::create_dir_all(claude_dir.join(subdir))?;
    }

    if static_only || !claude_available() {
        if !static_only {
            eprintln!("claude CLI not found. Installing generic templates.");
            eprintln!(
                "Re-run sipag init after installing Claude Code for project-specific setup.\n"
            );
        }
        return install_static_templates(&claude_dir, force);
    }

    // Generative: launch Claude to explore project and write customized files.
    let prompt = build_init_prompt(force);
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

fn build_init_prompt(force: bool) -> String {
    let force_instruction = if force {
        "Overwrite any existing files in .claude/."
    } else {
        "If .claude/ already contains customized files, ask before overwriting."
    };
    INIT_PROMPT
        .replace("{FORCE_INSTRUCTION}", force_instruction)
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
        .replace("{COMMAND_DISPATCH}", templates::COMMAND_DISPATCH)
        .replace("{COMMAND_REVIEW}", templates::COMMAND_REVIEW)
        .replace("{COMMAND_TRIAGE}", templates::COMMAND_TRIAGE)
        .replace("{HOOK_SAFETY_GATE_TOML}", templates::HOOK_SAFETY_GATE_TOML)
}

fn exec_claude(project_dir: &Path, prompt: &str) -> Result<()> {
    let err = Command::new("claude")
        .arg("--append-system-prompt")
        .arg(prompt)
        .arg("Set up Claude Code for this project. Start by exploring the project structure, then generate customized agents and commands.")
        .current_dir(project_dir)
        .exec();
    bail!("failed to exec claude: {err}")
}

fn install_static_templates(claude_dir: &Path, force: bool) -> Result<()> {
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
