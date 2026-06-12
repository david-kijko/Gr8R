use std::collections::HashMap;

use crate::api::schema::{
    Method, PluginActionInvokeParams, PluginActionListParams, PluginInvocationContext,
    PluginLinkParams, PluginListParams, PluginLogListParams, PluginPaneCloseParams,
    PluginPaneFocusParams, PluginPaneOpenParams, PluginPanePlacement, PluginSetEnabledParams,
    PluginUnlinkParams, Request, SplitDirection,
};

pub(super) fn run_plugin_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_plugin_help();
        return Ok(2);
    };

    match subcommand {
        "link" => plugin_link(&args[1..]),
        "list" => plugin_list(&args[1..]),
        "unlink" => plugin_unlink(&args[1..]),
        "enable" => plugin_set_enabled(&args[1..], true),
        "disable" => plugin_set_enabled(&args[1..], false),
        "action" => run_plugin_action_command(&args[1..]),
        "log" | "logs" => plugin_log_list(&args[1..]),
        "pane" => run_plugin_pane_command(&args[1..]),
        "help" | "--help" | "-h" => {
            print_plugin_help();
            Ok(0)
        }
        _ => {
            print_plugin_help();
            Ok(2)
        }
    }
}

fn plugin_link(args: &[String]) -> std::io::Result<i32> {
    let Some(path) = args.first() else {
        eprintln!("usage: herdr plugin link <path> [--disabled]");
        return Ok(2);
    };
    let mut enabled = true;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--disabled" => {
                enabled = false;
                index += 1;
            }
            "--enabled" => {
                enabled = true;
                index += 1;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    print_plugin_response(Method::PluginLink(PluginLinkParams {
        path: path.clone(),
        enabled,
    }))
}

fn plugin_list(args: &[String]) -> std::io::Result<i32> {
    let mut plugin_id = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--plugin" => plugin_id = Some(required_value(args, &mut index, "--plugin")?),
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    print_plugin_response(Method::PluginList(PluginListParams { plugin_id }))
}

fn plugin_unlink(args: &[String]) -> std::io::Result<i32> {
    let Some(plugin_id) = args.first() else {
        eprintln!("usage: herdr plugin unlink <plugin_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: herdr plugin unlink <plugin_id>");
        return Ok(2);
    }
    print_plugin_response(Method::PluginUnlink(PluginUnlinkParams {
        plugin_id: plugin_id.clone(),
    }))
}

fn plugin_set_enabled(args: &[String], enabled: bool) -> std::io::Result<i32> {
    let Some(plugin_id) = args.first() else {
        eprintln!(
            "usage: herdr plugin {} <plugin_id>",
            if enabled { "enable" } else { "disable" }
        );
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!(
            "usage: herdr plugin {} <plugin_id>",
            if enabled { "enable" } else { "disable" }
        );
        return Ok(2);
    }
    let params = PluginSetEnabledParams {
        plugin_id: plugin_id.clone(),
    };
    if enabled {
        print_plugin_response(Method::PluginEnable(params))
    } else {
        print_plugin_response(Method::PluginDisable(params))
    }
}

fn plugin_log_list(args: &[String]) -> std::io::Result<i32> {
    let mut plugin_id = None;
    let mut limit = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "list" if index == 0 => index += 1,
            "--plugin" => plugin_id = Some(required_value(args, &mut index, "--plugin")?),
            "--limit" => {
                let raw = required_value(args, &mut index, "--limit")?;
                limit =
                    Some(raw.parse::<usize>().map_err(|_| {
                        std::io::Error::other(format!("invalid --limit value: {raw}"))
                    })?);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    print_plugin_response(Method::PluginLogList(PluginLogListParams {
        plugin_id,
        limit,
    }))
}

fn run_plugin_action_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_plugin_action_help();
        return Ok(2);
    };

    match subcommand {
        "list" => plugin_action_list(&args[1..]),
        "invoke" => plugin_action_invoke(&args[1..]),
        "help" | "--help" | "-h" => {
            print_plugin_action_help();
            Ok(0)
        }
        _ => {
            print_plugin_action_help();
            Ok(2)
        }
    }
}

fn plugin_action_list(args: &[String]) -> std::io::Result<i32> {
    let mut plugin_id = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--plugin" => {
                plugin_id = Some(required_value(args, &mut index, "--plugin")?);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    print_plugin_response(Method::PluginActionList(PluginActionListParams {
        plugin_id,
    }))
}

fn plugin_action_invoke(args: &[String]) -> std::io::Result<i32> {
    let Some(action_id) = args.first() else {
        eprintln!("usage: herdr plugin action invoke <action_id> [--plugin ID]");
        return Ok(2);
    };
    let mut plugin_id = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--plugin" => {
                plugin_id = Some(required_value(args, &mut index, "--plugin")?);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    print_plugin_response(Method::PluginActionInvoke(PluginActionInvokeParams {
        action_id: action_id.clone(),
        plugin_id,
        context: Some(PluginInvocationContext {
            workspace_id: None,
            workspace_label: None,
            workspace_cwd: None,
            worktree: None,
            tab_id: None,
            tab_label: None,
            focused_pane_id: None,
            focused_pane_cwd: None,
            focused_pane_agent: None,
            focused_pane_status: None,
            selected_text: None,
            invocation_source: Some("cli".into()),
            correlation_id: None,
            clicked_url: None,
            link_handler_id: None,
        }),
    }))
}

fn run_plugin_pane_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_plugin_pane_help();
        return Ok(2);
    };

    match subcommand {
        "open" => plugin_pane_open(&args[1..]),
        "focus" => plugin_pane_focus(&args[1..]),
        "close" => plugin_pane_close(&args[1..]),
        "help" | "--help" | "-h" => {
            print_plugin_pane_help();
            Ok(0)
        }
        _ => {
            print_plugin_pane_help();
            Ok(2)
        }
    }
}

fn plugin_pane_open(args: &[String]) -> std::io::Result<i32> {
    let mut plugin_id = None;
    let mut entrypoint = None;
    let mut placement = None;
    let mut workspace_id = None;
    let mut target_pane_id = None;
    let mut direction = None;
    let mut cwd = None;
    let mut focus = true;
    let mut env = HashMap::new();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--plugin" => plugin_id = Some(required_value(args, &mut index, "--plugin")?),
            "--entrypoint" => entrypoint = Some(required_value(args, &mut index, "--entrypoint")?),
            "--placement" => {
                placement = Some(parse_pane_placement(&required_value(
                    args,
                    &mut index,
                    "--placement",
                )?)?);
            }
            "--workspace" => workspace_id = Some(required_value(args, &mut index, "--workspace")?),
            "--target-pane" => {
                target_pane_id = Some(required_value(args, &mut index, "--target-pane")?);
            }
            "--direction" => {
                direction = Some(parse_split_direction(&required_value(
                    args,
                    &mut index,
                    "--direction",
                )?)?);
            }
            "--cwd" => cwd = Some(required_value(args, &mut index, "--cwd")?),
            "--env" => {
                let value = required_value(args, &mut index, "--env")?;
                let (key, value) = match super::parse_env_assignment(&value) {
                    Ok(pair) => pair,
                    Err(err) => {
                        eprintln!("{err}");
                        return Ok(2);
                    }
                };
                env.insert(key, value);
            }
            "--focus" => {
                focus = true;
                index += 1;
            }
            "--no-focus" => {
                focus = false;
                index += 1;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(plugin_id) = plugin_id else {
        eprintln!("missing required --plugin");
        return Ok(2);
    };
    let Some(entrypoint) = entrypoint else {
        eprintln!("missing required --entrypoint");
        return Ok(2);
    };

    print_plugin_response(Method::PluginPaneOpen(PluginPaneOpenParams {
        plugin_id,
        entrypoint,
        placement,
        workspace_id,
        target_pane_id,
        direction,
        cwd,
        focus,
        env,
    }))
}

fn plugin_pane_focus(args: &[String]) -> std::io::Result<i32> {
    let Some(pane_id) = args.first() else {
        eprintln!("usage: herdr plugin pane focus <pane_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: herdr plugin pane focus <pane_id>");
        return Ok(2);
    }
    print_plugin_response(Method::PluginPaneFocus(PluginPaneFocusParams {
        pane_id: super::normalize_pane_id(pane_id),
    }))
}

fn plugin_pane_close(args: &[String]) -> std::io::Result<i32> {
    let Some(pane_id) = args.first() else {
        eprintln!("usage: herdr plugin pane close <pane_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: herdr plugin pane close <pane_id>");
        return Ok(2);
    }
    print_plugin_response(Method::PluginPaneClose(PluginPaneCloseParams {
        pane_id: super::normalize_pane_id(pane_id),
    }))
}

fn required_value(args: &[String], index: &mut usize, flag: &str) -> std::io::Result<String> {
    let Some(value) = args.get(*index + 1) else {
        eprintln!("missing value for {flag}");
        return Err(std::io::Error::other(format!("missing value for {flag}")));
    };
    *index += 2;
    Ok(value.clone())
}

fn parse_pane_placement(value: &str) -> std::io::Result<PluginPanePlacement> {
    match value {
        "overlay" => Ok(PluginPanePlacement::Overlay),
        "split" => Ok(PluginPanePlacement::Split),
        "tab" => Ok(PluginPanePlacement::Tab),
        "zoomed" | "fullscreen" => Ok(PluginPanePlacement::Zoomed),
        _ => Err(std::io::Error::other(format!(
            "invalid pane placement: {value}"
        ))),
    }
}

fn parse_split_direction(value: &str) -> std::io::Result<SplitDirection> {
    match value {
        "right" => Ok(SplitDirection::Right),
        "down" => Ok(SplitDirection::Down),
        _ => Err(std::io::Error::other(format!(
            "invalid split direction: {value}"
        ))),
    }
}

fn print_plugin_response(method: Method) -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:plugin".into(),
        method,
    })?)
}

fn print_plugin_help() {
    eprintln!("herdr plugin commands:");
    eprintln!("  herdr plugin link <path> [--disabled]");
    eprintln!("  herdr plugin list [--plugin ID]");
    eprintln!("  herdr plugin unlink <plugin_id>");
    eprintln!("  herdr plugin enable <plugin_id>");
    eprintln!("  herdr plugin disable <plugin_id>");
    eprintln!("  herdr plugin action <list|invoke>");
    eprintln!("  herdr plugin log list [--plugin ID] [--limit N]");
    eprintln!("  herdr plugin pane <open|focus|close>");
}

fn print_plugin_action_help() {
    eprintln!("herdr plugin action commands:");
    eprintln!("  herdr plugin action list [--plugin ID]");
    eprintln!("  herdr plugin action invoke <action_id> [--plugin ID]");
}

fn print_plugin_pane_help() {
    eprintln!("herdr plugin pane commands:");
    eprintln!("  herdr plugin pane open --plugin ID --entrypoint ID [--placement overlay|split|tab|zoomed] [--workspace ID] [--target-pane PANE] [--direction right|down] [--cwd PATH] [--env KEY=VALUE] [--focus|--no-focus]");
    eprintln!("  herdr plugin pane focus <pane_id>");
    eprintln!("  herdr plugin pane close <pane_id>");
}
