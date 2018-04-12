use errors::*;
use ex::{self, ExCapLints, ExCrateSelect, ExMode, ExOpts};
use futures::future;
use futures::prelude::*;
use hyper::StatusCode;
use hyper::server::{Request, Response};
use ring;
use serde_json;
use server::Data;
use server::github::{EventIssueComment, Issue};
use server::http::{Context, ResponseExt, ResponseFuture};
use server::messages::{Label, Message};
use std::sync::Arc;
use toolchain::Toolchain;
use util;

#[derive(Debug, Default)]
struct EditArguments {
    run: Option<bool>,
    name: Option<String>,
    start: Option<Toolchain>,
    end: Option<Toolchain>,
    mode: Option<ExMode>,
    crates: Option<ExCrateSelect>,
    lints: Option<ExCapLints>,
    p: Option<i32>,
}

fn process_webhook(payload: &str, signature: &str, event: &str, data: &Data) -> Result<()> {
    if !verify_signature(&data.tokens.bot.webhooks_secret, payload, signature) {
        bail!("invalid signature for the webhook!");
    }

    match event {
        "ping" => info!("the webhook is configured correctly!"),
        "issue_comment" => {
            let p: EventIssueComment = serde_json::from_str(payload)?;
            if let Err(e) = process_command(
                &p.sender.login,
                &p.comment.body,
                &p.comment.issue_url,
                &p.issue,
                data,
            ) {
                Message::new()
                    .line("rotating_light", format!("**Error:** {}", e))
                    .note(
                        "sos",
                        "If you have any trouble with Crater please ping **`@rust-lang/infra`**!",
                    )
                    .send(&p.comment.issue_url, data)?;
            }
        }
        e => bail!("invalid event received: {}", e),
    }

    Ok(())
}

fn process_command(
    sender: &str,
    body: &str,
    issue_url: &str,
    issue: &Issue,
    data: &Data,
) -> Result<()> {
    let start = format!("@{} ", data.bot_username);
    for line in body.lines() {
        if line.starts_with(&start) {
            let command = line.split(' ').skip(1).collect::<Vec<_>>();
            if command.is_empty() {
                continue;
            }

            if !data.config.server.bot_acl.contains(sender) {
                Message::new()
                    .line(
                        "lock",
                        "**Error:** you're not allowed to interact with this bot.",
                    )
                    .note(
                        "key",
                        "If you are a member of the Rust team and need access, [add yourself to \
                         the whitelist](\
                         https://github.com/rust-lang-nursery/crater/blob/master/config.toml).",
                    )
                    .send(issue_url, data)?;
                return Ok(());
            }

            info!("user @{} sent command: {}", sender, command.join(" "));

            if command.len() == 1 && command[0] == "ping" {
                Message::new()
                    .line("ping_pong", "**Pong!**")
                    .send(issue_url, data)?;
                break;
            }

            let args = parse_edit_arguments(&command)?;

            let name = if let Some(name) = args.name {
                name
            } else if let Some(default) = default_experiment_name(issue) {
                default
            } else {
                bail!("missing experiment name!");
            };

            let mut experiments = data.experiments.lock().unwrap();

            match args.run {
                // Create the experiment
                Some(true) => {
                    if experiments.exists(&name) {
                        bail!("an experiment named **`{}`** already exists!", name);
                    }

                    let start = args.start.ok_or_else(|| "missing start toolchain")?;
                    let end = args.end.ok_or_else(|| "missing end toolchain")?;
                    let mode = args.mode.unwrap_or(ExMode::BuildAndTest);
                    let crates = args.crates.unwrap_or(ExCrateSelect::Full);
                    let cap_lints = args.lints.ok_or_else(|| "missing lints option")?;
                    let priority = args.p.unwrap_or(0);

                    experiments.create(
                        ExOpts {
                            name: name.clone(),
                            toolchains: vec![start, end],
                            mode,
                            crates,
                            cap_lints,
                        },
                        &data.config,
                        issue_url,
                        priority,
                    )?;

                    Message::new()
                        .line(
                            "ok_hand",
                            format!("Experiment **`{}`** created and queued.", name),
                        )
                        .set_label(Label::ExperimentQueued)
                        .send(issue_url, data)?;
                }
                // Delete the experiment
                Some(false) => {
                    if !experiments.exists(&name) {
                        bail!("an experiment named **`{}`** doesn't exist!", name);
                    }

                    experiments.delete(&name)?;

                    Message::new()
                        .line("wastebasket", format!("Experiment **`{}`** deleted!", name))
                        .set_label(Label::ExperimentCompleted)
                        .send(issue_url, data)?;
                }
                // Edit the experiment
                None => {
                    if !experiments.exists(&name) {
                        bail!("an experiment named **`{}`** doesn't exist!", name);
                    }

                    let mut changed = false;
                    let mut info = experiments.edit_data(&name).unwrap();

                    if let Some(start) = args.start {
                        info.experiment.toolchains[0] = start;
                        changed = true;
                    }
                    if let Some(end) = args.end {
                        info.experiment.toolchains[1] = end;
                        changed = true;
                    }
                    if let Some(mode) = args.mode {
                        info.experiment.mode = mode;
                        changed = true;
                    }
                    if let Some(crates) = args.crates {
                        info.experiment.crates = ex::get_crates(crates, &data.config)?;
                        changed = true;
                    }
                    if let Some(priority) = args.p {
                        info.server_data.priority = priority;
                        changed = true;
                    }

                    if changed {
                        info.save()?;

                        Message::new()
                            .line(
                                "memo",
                                format!("Configuration of the **`{}`** experiment changed.", name),
                            )
                            .send(issue_url, data)?;
                    } else {
                        Message::new()
                            .line("warning", "No changes requested.")
                            .send(issue_url, data)?;
                    }
                }
            }

            break;
        }
    }

    Ok(())
}

fn default_experiment_name(issue: &Issue) -> Option<String> {
    if issue.pull_request.is_some() {
        Some(format!("pr-{}", issue.number))
    } else {
        None
    }
}

fn parse_edit_arguments(args: &[&str]) -> Result<EditArguments> {
    macro_rules! parse_edit_arguments {
        ($args:expr, bools: [$($bool:ident),*], args: [$($arg:ident),*]) => {{
            let mut result = EditArguments::default();

            for arg in args {
                if false {}
                $(
                    else if arg == &stringify!($bool) {
                        result.$bool = Some(true);
                    }
                    else if arg == &concat!(stringify!($bool), "-") {
                        result.$bool = Some(false);
                    }
                )*
                $(
                    else if arg.starts_with(concat!(stringify!($arg), "=")) {
                        result.$arg = Some(arg.splitn(2, '=').skip(1).next().unwrap().parse()?);
                    }
                )*
                else {
                    bail!("unknown argument: {}", arg);
                }
            }

            Ok(result)
        }}
    }

    parse_edit_arguments!(args, bools: [run], args: [name, start, end, mode, crates, lints, p])
}

fn verify_signature(secret: &str, payload: &str, raw_signature: &str) -> bool {
    // The signature must have a =
    if !raw_signature.contains('=') {
        return false;
    }

    // Split the raw signature to get the algorithm and the signature
    let splitted: Vec<&str> = raw_signature.split('=').collect();
    let algorithm = &splitted[0];
    let hex_signature = splitted
        .iter()
        .skip(1)
        .map(|i| *i)
        .collect::<Vec<&str>>()
        .join("=");

    // Convert the signature from hex
    let signature = if let Ok(converted) = util::from_hex(&hex_signature) {
        converted
    } else {
        // This is not hex
        return false;
    };

    // Get the correct digest
    let digest = match *algorithm {
        "sha1" => &ring::digest::SHA1,
        _ => {
            // Unknown digest, return false
            return false;
        }
    };

    // Verify the HMAC signature
    let key = ring::hmac::VerificationKey::new(digest, secret.as_bytes());
    ring::hmac::verify(&key, payload.as_bytes(), &signature).is_ok()
}

macro_rules! headers {
    ($req:expr => { $($ident:ident: $name:expr,)* }) => {
        $(
            let option = $req.headers()
                .get_raw($name)
                .and_then(|h| h.one())
                .map(|s| String::from_utf8_lossy(s).to_string());

            let $ident = if let Some(some) = option {
                some
            } else {
                error!("missing header in the webhook: {}", $name);

                return Response::json(&json!({
                    "error": format!("missing header: {}", $name),
                })).unwrap().with_status(StatusCode::BadRequest).as_future();
            };
        )*
    }
}

pub fn handle(req: Request, data: Arc<Data>, ctx: Arc<Context>) -> ResponseFuture {
    headers!(req => {
        signature: "X-Hub-Signature",
        event: "X-GitHub-Event",
    });

    Box::new(req.body().concat2().and_then(move |body| {
        let body = String::from_utf8_lossy(&body.iter().cloned().collect::<Vec<u8>>()).to_string();

        ctx.handle.spawn(ctx.pool.spawn_fn(move || {
            if let Err(err) = process_webhook(&body, &signature, &event, &data) {
                error!("error while processing webhook: {}", err);
            }

            future::ok(())
        }));

        Response::text("OK\n").as_future()
    }))
}

#[cfg(test)]
mod tests {
    use super::default_experiment_name;
    use server::github;

    #[test]
    fn test_default_experiment_name() {
        // With simple issues no default should be used
        let issue = github::Issue {
            number: 1,
            labels: Vec::new(),
            pull_request: None,
        };
        assert!(default_experiment_name(&issue).is_none());

        // With pull requests pr-{number} should be used
        let pr = github::Issue {
            number: 2,
            labels: Vec::new(),
            pull_request: Some(github::PullRequest {
                html_url: String::new(),
            }),
        };
        assert_eq!(default_experiment_name(&pr).unwrap().as_str(), "pr-2");
    }
}
