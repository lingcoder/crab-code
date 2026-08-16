mod acp_mode;
mod agent;
mod args;
#[allow(clippy::doc_markdown)]
mod commands;
mod completions;
mod deep_link;
mod installer;
mod output;
mod repl;

use clap::Parser;

use crate::args::{Cli, CliCommand, SessionAction};

/// If `argv[1]` is a `crab-cli://` deep-link URL, rewrite it into an
/// equivalent set of CLI arguments that clap can parse. Returns `None`
/// if the first argument is not a deep link (so the caller should use
/// the normal argv).
///
/// Errors when a recognisable deep link is malformed or carries an
/// unsafe identifier — preferring an explicit failure over silent
/// fall-through prevents user confusion when an OS hands us garbage.
fn rewrite_deep_link_argv(argv: Vec<String>) -> anyhow::Result<Option<Vec<String>>> {
    let mut iter = argv.into_iter();
    let Some(program) = iter.next() else {
        return Ok(None);
    };
    let Some(first) = iter.next() else {
        return Ok(None);
    };
    if !first.starts_with("crab-cli://") {
        return Ok(None);
    }
    let action = crate::deep_link::parse_deep_link(&first)
        .ok_or_else(|| anyhow::anyhow!("invalid crab-cli:// URL: {first}"))?;
    Ok(Some(deep_link_to_argv(program, action)?))
}

/// Convert a parsed [`DeepLinkAction`] into a clap-friendly argv vector.
/// Identifier arguments (`session_id`, `plugin_name`) are validated to
/// contain only safe characters; free-form text is passed after `--` so
/// clap will not interpret leading dashes as flags.
fn deep_link_to_argv(
    program: String,
    action: crate::deep_link::DeepLinkAction,
) -> anyhow::Result<Vec<String>> {
    use crate::deep_link::DeepLinkAction;
    match action {
        DeepLinkAction::OpenSession { session_id } => {
            ensure_safe_id(&session_id, "session id")?;
            Ok(vec![program, "session".into(), "resume".into(), session_id])
        }
        DeepLinkAction::InstallPlugin { plugin_name } => {
            ensure_safe_id(&plugin_name, "plugin name")?;
            Ok(vec![
                program,
                "plugin".into(),
                "install".into(),
                plugin_name,
            ])
        }
        DeepLinkAction::RunCommand { command } => {
            if command.chars().any(char::is_control) {
                anyhow::bail!("deep-link command contains control characters");
            }
            Ok(vec![program, "--".into(), command])
        }
    }
}

/// Lexically validate an identifier from a deep link. Accepts ASCII
/// alphanumerics, `-`, `_`, and `.` up to 128 chars; rejects anything
/// that could encode shell metacharacters or path traversal.
fn ensure_safe_id(id: &str, kind: &str) -> anyhow::Result<()> {
    if id.is_empty() {
        anyhow::bail!("deep-link {kind} is empty");
    }
    if id.len() > 128 {
        anyhow::bail!("deep-link {kind} is too long");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        anyhow::bail!("deep-link {kind} contains invalid characters: {id}");
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = match rewrite_deep_link_argv(std::env::args().collect())? {
        Some(rewritten) => Cli::parse_from(rewritten),
        None => Cli::parse(),
    };

    // Apply the sandbox on/off switch before any tool can spawn a subprocess.
    if let Some(mode) = &cli.sandbox {
        let mode = mode
            .parse::<crab_sandbox::SandboxMode>()
            .map_err(|e| anyhow::anyhow!(e))?;
        crab_sandbox::set_mode(mode);
    }

    // ACP mode: bypass all interactive / print-mode plumbing and run
    // as a JSON-RPC stdio child process for the spawning editor. All
    // other CLI flags are ignored.
    if cli.acp {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return rt.block_on(acp_mode::run());
    }

    // Handle subcommands that don't need LLM/MCP (fast paths).
    // `Session Resume` validates then falls through to the shared runtime below.
    let mut subcommand_resume_id: Option<String> = None;
    if let Some(command) = &cli.command {
        match command {
            CliCommand::Session {
                action: SessionAction::Resume { id },
            } => {
                let _ = commands::session::validate_resume_id(id)?;
                subcommand_resume_id = Some(id.clone());
                // fall through to the shared runtime
            }
            _ => {
                return match command {
                    CliCommand::Config { action } => {
                        commands::config::run(action, &cli.config_override)
                    }
                    CliCommand::Serve(args) => {
                        let rt = tokio::runtime::Runtime::new()?;
                        rt.block_on(commands::serve::run(args))
                    }
                    CliCommand::Session { action } => match action {
                        SessionAction::List => commands::session::list_sessions(),
                        SessionAction::Show { id } => commands::session::show_session(id),
                        SessionAction::Resume { .. } => unreachable!(),
                        SessionAction::Delete { id } => commands::session::delete_session(id),
                        SessionAction::Search { keyword } => {
                            commands::session::search_sessions(keyword)
                        }
                        SessionAction::Export { id, format } => {
                            commands::session::export_session(id, format)
                        }
                        SessionAction::Stats { id } => commands::session::show_stats(id),
                    },
                    CliCommand::Auth { action } => commands::auth::run(action),
                    CliCommand::Doctor => commands::doctor::run(),
                    CliCommand::Update { action } => match action {
                        Some(a) => commands::update::run(a),
                        None => commands::update::run_default(),
                    },
                    CliCommand::Plugin { action } => commands::plugin::run(action),
                    CliCommand::Agents => commands::agents::run(),
                    CliCommand::Permissions { action } => commands::permissions::run(action),
                    CliCommand::Completion { shell } => {
                        let mut cmd = <Cli as clap::CommandFactory>::command();
                        crate::completions::generate_completions(
                            *shell,
                            &mut cmd,
                            &mut std::io::stdout(),
                        )?;
                        Ok(())
                    }
                };
            }
        }
    }

    let resume_id = subcommand_resume_id.or_else(|| cli.resume.clone());
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(agent::run(&cli, resume_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_rewrite_ignores_regular_argv() {
        let argv = vec!["crab".into(), "hello".into()];
        let out = rewrite_deep_link_argv(argv).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn deep_link_rewrite_passes_through_without_args() {
        let argv = vec!["crab".into()];
        let out = rewrite_deep_link_argv(argv).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn deep_link_rewrite_open_session_maps_to_resume() {
        let argv = vec!["crab".into(), "crab-cli://open-session/my-sess-42".into()];
        let out = rewrite_deep_link_argv(argv).unwrap().unwrap();
        assert_eq!(out, vec!["crab", "session", "resume", "my-sess-42"]);
    }

    #[test]
    fn deep_link_rewrite_install_plugin_maps_to_plugin_install() {
        let argv = vec!["crab".into(), "crab-cli://install-plugin/my-plugin".into()];
        let out = rewrite_deep_link_argv(argv).unwrap().unwrap();
        assert_eq!(out, vec!["crab", "plugin", "install", "my-plugin"]);
    }

    #[test]
    fn deep_link_rewrite_run_command_prepends_dash_dash() {
        let argv = vec!["crab".into(), "crab-cli://run/hello%20world".into()];
        let out = rewrite_deep_link_argv(argv).unwrap().unwrap();
        assert_eq!(out, vec!["crab", "--", "hello world"]);
    }

    #[test]
    fn deep_link_rewrite_rejects_shell_metacharacters_in_id() {
        let argv = vec![
            "crab".into(),
            "crab-cli://open-session/%3Brm%20-rf%20%2F".into(),
        ];
        let err = rewrite_deep_link_argv(argv).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn deep_link_rewrite_rejects_malformed_url() {
        let argv = vec!["crab".into(), "crab-cli://nonsense".into()];
        let err = rewrite_deep_link_argv(argv).unwrap_err();
        assert!(err.to_string().contains("invalid crab-cli"));
    }

    #[test]
    fn deep_link_rewrite_rejects_control_chars_in_command() {
        let argv = vec!["crab".into(), "crab-cli://run/bad%00cmd".into()];
        let err = rewrite_deep_link_argv(argv).unwrap_err();
        assert!(err.to_string().contains("control"));
    }
}
