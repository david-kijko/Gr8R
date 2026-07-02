use crate::api::schema::IntegrationTarget;

pub(super) fn run_integration_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_integration_help();
        return Ok(2);
    };

    match subcommand {
        "install" => integration_install(&args[1..]),
        "uninstall" => integration_uninstall(&args[1..]),
        "status" => integration_status(&args[1..]),
        "help" | "--help" | "-h" => {
            print_integration_help();
            Ok(0)
        }
        _ => {
            print_integration_help();
            Ok(2)
        }
    }
}

fn integration_status(args: &[String]) -> std::io::Result<i32> {
    let outdated_only = match args {
        [] => false,
        [flag] if flag == "--outdated-only" => true,
        _ => {
            eprintln!("usage: gr8r integration status [--outdated-only]");
            return Ok(2);
        }
    };

    if outdated_only {
        crate::integration::print_outdated_update_notice();
        return Ok(0);
    }

    for status in crate::integration::installed_integration_statuses() {
        let target = crate::integration::integration_target_label(status.target);
        let version = match status.installed_version {
            Some(version) => format!("v{version}"),
            None => "legacy".to_string(),
        };
        let state = match status.state {
            crate::integration::IntegrationStatusKind::NotInstalled => "not installed".to_string(),
            crate::integration::IntegrationStatusKind::Current => {
                format!("current ({version})")
            }
            crate::integration::IntegrationStatusKind::Outdated => {
                format!("outdated ({version} < v{})", status.expected_version)
            }
        };
        println!("{target}: {state} ({})", status.path.display());
    }

    Ok(0)
}

fn integration_install(args: &[String]) -> std::io::Result<i32> {
    let Some(target) = parse_integration_target(args, "install")? else {
        return Ok(2);
    };

    match crate::integration::install_target(target) {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn integration_uninstall(args: &[String]) -> std::io::Result<i32> {
    let Some(target) = parse_integration_target(args, "uninstall")? else {
        return Ok(2);
    };

    match crate::integration::uninstall_target(target) {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn print_integration_messages(messages: Vec<String>) {
    for message in messages {
        println!("{message}");
    }
}

fn parse_integration_target(
    args: &[String],
    action: &str,
) -> std::io::Result<Option<IntegrationTarget>> {
    let Some(target) = args.first().map(|arg| arg.as_str()) else {
        eprintln!(
            "usage: gr8r integration {action} <pi|omp|claude|codex|copilot|devin|droid|kimi|opencode|kilo|hermes|qodercli|cursor>"
        );
        return Ok(None);
    };
    if args.len() != 1 {
        eprintln!(
            "usage: gr8r integration {action} <pi|omp|claude|codex|copilot|devin|droid|kimi|opencode|kilo|hermes|qodercli|cursor>"
        );
        return Ok(None);
    }

    let parsed = match target {
        "pi" => IntegrationTarget::Pi,
        "omp" => IntegrationTarget::Omp,
        "claude" => IntegrationTarget::Claude,
        "codex" => IntegrationTarget::Codex,
        "copilot" => IntegrationTarget::Copilot,
        "devin" => IntegrationTarget::Devin,
        "droid" => IntegrationTarget::Droid,
        "kimi" => IntegrationTarget::Kimi,
        "opencode" => IntegrationTarget::Opencode,
        "kilo" => IntegrationTarget::Kilo,
        "hermes" => IntegrationTarget::Hermes,
        "qodercli" => IntegrationTarget::Qodercli,
        "cursor" => IntegrationTarget::Cursor,
        _ => {
            eprintln!("unknown integration target: {target}");
            eprintln!(
                "currently supported: pi, omp, claude, codex, copilot, devin, droid, kimi, opencode, kilo, hermes, qodercli, cursor"
            );
            return Ok(None);
        }
    };

    Ok(Some(parsed))
}

fn print_integration_help() {
    eprintln!("gr8r integration commands:");
    eprintln!("  gr8r integration install pi");
    eprintln!("  gr8r integration install omp");
    eprintln!("  gr8r integration install claude");
    eprintln!("  gr8r integration install codex");
    eprintln!("  gr8r integration install copilot");
    eprintln!("  gr8r integration install devin");
    eprintln!("  gr8r integration install droid");
    eprintln!("  gr8r integration install kimi");
    eprintln!("  gr8r integration install opencode");
    eprintln!("  gr8r integration install kilo");
    eprintln!("  gr8r integration install hermes");
    eprintln!("  gr8r integration install qodercli");
    eprintln!("  gr8r integration install cursor");
    eprintln!("  gr8r integration uninstall pi");
    eprintln!("  gr8r integration uninstall omp");
    eprintln!("  gr8r integration uninstall claude");
    eprintln!("  gr8r integration uninstall codex");
    eprintln!("  gr8r integration uninstall copilot");
    eprintln!("  gr8r integration uninstall devin");
    eprintln!("  gr8r integration uninstall droid");
    eprintln!("  gr8r integration uninstall kimi");
    eprintln!("  gr8r integration uninstall opencode");
    eprintln!("  gr8r integration uninstall kilo");
    eprintln!("  gr8r integration uninstall hermes");
    eprintln!("  gr8r integration uninstall qodercli");
    eprintln!("  gr8r integration uninstall cursor");
    eprintln!("  gr8r integration status [--outdated-only]");
}
