use errors::*;
use ex::Experiment;
use futures::{future, Future, Stream};
use hyper::server::{Request, Response};
use serde_json;
use server::Data;
use server::api_types::{AgentConfig, ApiResponse};
use server::auth::AuthDetails;
use server::experiments::Status;
use server::http::{Context, ResponseExt, ResponseFuture};
use server::messages::{Label, Message};
use server::results::{self, ResultsDB, TaskResult};
use std::sync::Arc;

api_endpoint!(config: |_body, data, auth: AuthDetails| -> AgentConfig {
    Ok(ApiResponse::Success {
        result: AgentConfig {
            agent_name: auth.name,
            crater_config: data.config.clone(),
        },
    })
}, config_inner);

api_endpoint!(next_ex: |_body, data, auth: AuthDetails| -> Option<Experiment> {
    let next = data.experiments.next(&auth.name)?;
    if let Some((new, ex)) = next {
        if new {
            if let Some(ref github_issue) = ex.server_data.github_issue {
                Message::new()
                    .line(
                        "construction",
                        format!(
                            "Experiment **`{}`** is now **running** on agent `{}`.",
                            ex.experiment.name,
                            auth.name,
                        ),
                    )
                    .send(&github_issue.api_url, &data)?;
            }
        }

        Ok(ApiResponse::Success { result: Some(ex.experiment) })
    } else {
        Ok(ApiResponse::Success { result: None })
    }
}, next_ex_inner);

api_endpoint!(complete_ex: |_body, data, auth: AuthDetails| -> bool {
    let mut ex = data.experiments
        .run_by_agent(&auth.name)?
        .ok_or("no experiment run by this agent")?;

    ex.set_status(&data.db, Status::Completed)?;

    let name = ex.experiment.name;

    info!("experiment {} completed, generating report...", name);
    let report_url = results::generate_report(&data.db, &name, &data.config, &data.tokens)?;
    info!("report for the experiment {} generated successfully!", name);

    if let Some(ref github_issue) = ex.server_data.github_issue {
        Message::new()
            .line("tada", format!("Experiment **`{}`** is completed!", name))
            .line("newspaper", format!("[Open the full report]({}).", report_url))
            .note(
                "warning",
                "If you notice any spurious failure [please add them to the \
                blacklist](https://github.com/rust-lang-nursery/crater/blob/master/config.toml)!",
            )
            .set_label(Label::ExperimentCompleted)
            .send(&github_issue.api_url, &data)?;
    }

    Ok(ApiResponse::Success { result: true })
}, complete_ex_inner);

api_endpoint!(record_result: |body, data, auth: AuthDetails| -> bool {
    let result: TaskResult = serde_json::from_str(&body)?;

    let experiment = data.experiments.run_by_agent(&auth.name)?.ok_or("no experiment run by this agent")?;

    info!(
        "receiving a result from agent {} (ex: {}, tc: {}, crate: {})",
        auth.name,
        experiment.experiment.name,
        result.toolchain.to_string(),
        result.krate
    );

    let db = ResultsDB::new(&data.db);
    db.store(&experiment.experiment, &result)?;

    Ok(ApiResponse::Success { result: true })
}, record_result_inner);

api_endpoint!(heartbeat: |_body, data, auth: AuthDetails| -> bool {
    data.agents.record_heartbeat(&auth.name)?;
    Ok(ApiResponse::Success { result: true })
}, heartbeat_inner);
