use crate::experiments::{Assignee, Experiment, ExperimentChunk, Status};
use crate::prelude::*;
use crate::results::{DatabaseDB, ProgressData};
use crate::server::api_types::{AgentConfig, ApiResponse};
use crate::server::auth::{auth_filter, AuthDetails, TokenType};
use crate::server::messages::Message;
use crate::server::{Data, HttpError};
use failure::Compat;
use http::{Response, StatusCode};
use hyper::Body;
use std::collections::HashMap;
use std::sync::Arc;
use warp::{self, Filter, Rejection};

pub fn routes(
    data: Arc<Data>,
) -> impl Filter<Extract = (Response<Body>,), Error = Rejection> + Clone {
    let data_cloned = data.clone();
    let data_filter = warp::any().map(move || data_cloned.clone());

    let config = warp::get2()
        .and(warp::path("config"))
        .and(warp::path::end())
        .and(data_filter.clone())
        .and(auth_filter(data.clone(), TokenType::Agent))
        .map(endpoint_config);

    let next_experiment = warp::get2()
        .and(warp::path("next-experiment-chunk"))
        .and(warp::path::end())
        .and(data_filter.clone())
        .and(auth_filter(data.clone(), TokenType::Agent))
        .map(endpoint_next_experiment_chunk);

    let complete_experiment = warp::post2()
        .and(warp::path("complete-experiment-chunk"))
        .and(warp::path::end())
        .and(data_filter.clone())
        .and(auth_filter(data.clone(), TokenType::Agent))
        .map(endpoint_complete_experiment_chunk);

    let record_progress = warp::post2()
        .and(warp::path("record-progress"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(data_filter.clone())
        .and(auth_filter(data.clone(), TokenType::Agent))
        .map(endpoint_record_progress);

    let heartbeat = warp::post2()
        .and(warp::path("heartbeat"))
        .and(warp::path::end())
        .and(data_filter.clone())
        .and(auth_filter(data.clone(), TokenType::Agent))
        .map(endpoint_heartbeat);

    let error = warp::post2()
        .and(warp::path("error"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(data_filter.clone())
        .and(auth_filter(data.clone(), TokenType::Agent))
        .map(endpoint_error);

    warp::any()
        .and(
            config
                .or(next_experiment)
                .unify()
                .or(complete_experiment)
                .unify()
                .or(record_progress)
                .unify()
                .or(heartbeat)
                .unify()
                .or(error)
                .unify(),
        )
        .map(handle_results)
        .recover(handle_errors)
        .unify()
}

fn endpoint_config(data: Arc<Data>, auth: AuthDetails) -> Fallible<Response<Body>> {
    Ok(ApiResponse::Success {
        result: AgentConfig {
            agent_name: auth.name,
            crater_config: data.config.clone(),
        },
    }
    .into_response()?)
}

fn endpoint_next_experiment_chunk(data: Arc<Data>, auth: AuthDetails) -> Fallible<Response<Body>> {
    let next = ExperimentChunk::next(&data.db, &Assignee::Agent(auth.name.clone()))?;

    let result = if let Some((_new, mut ex)) = next {
        let mut parent = Experiment::get(&data.db, &ex.parent_name)?
            .ok_or_else(|| err_msg("no experiment with this name"))?;

        if parent.status != Status::Running && parent.status != Status::Failed {
            parent.set_status(&data.db, Status::Running)?;

            if let Some(ref github_issue) = parent.github_issue {
                Message::new()
                    .line(
                        "construction",
                        format!("Experiment **`{}`** is now **running**.", parent.name,),
                    )
                    .send(&github_issue.api_url, &data)?;
            }
        }

        ex.remove_completed_crates(&data.db)?;
        Some(ex)
    } else {
        None
    };

    Ok(ApiResponse::Success { result }.into_response()?)
}

fn endpoint_complete_experiment_chunk(
    data: Arc<Data>,
    auth: AuthDetails,
) -> Fallible<Response<Body>> {
    let mut chunk = ExperimentChunk::run_by(&data.db, &Assignee::Agent(auth.name.clone()))?
        .ok_or_else(|| err_msg("no experiment chunk run by this agent"))?;
    let mut ex = Experiment::get(&data.db, &chunk.parent_name)?
        .ok_or_else(|| err_msg("no experiment with this name"))?;

    chunk.set_status(&data.db, Status::Completed)?;
    info!("experiment chunk {} completed", chunk.name);
    ex.complete_children(&data.db)?;
    if ex.children == 0 {
        info!("experiment {} completed, marked as NeedsReport", ex.name);
    }

    data.reports_worker.wake(); // Ensure the reports worker is awake

    Ok(ApiResponse::Success { result: true }.into_response()?)
}

fn endpoint_record_progress(
    result: ProgressData,
    data: Arc<Data>,
    auth: AuthDetails,
) -> Fallible<Response<Body>> {
    let chunk = ExperimentChunk::run_by(&data.db, &Assignee::Agent(auth.name.clone()))?
        .ok_or_else(|| err_msg("no experiment chunk run by this agent"))?;

    let ex = Experiment::get(&data.db, &chunk.parent_name)?
        .ok_or_else(|| err_msg("no experiment with this name"))?;

    info!(
        "received progress on experiment {} from agent {}",
        ex.name, auth.name,
    );

    let db = DatabaseDB::new(&data.db);
    //let old = db.load_all_results(&ex)?;
    //result.merge(old);
    db.store(&ex, &result)?;

    Ok(ApiResponse::Success { result: true }.into_response()?)
}

fn endpoint_heartbeat(data: Arc<Data>, auth: AuthDetails) -> Fallible<Response<Body>> {
    if let Some(rev) = auth.git_revision {
        data.agents.set_git_revision(&auth.name, &rev)?;
    }

    data.agents.record_heartbeat(&auth.name)?;
    Ok(ApiResponse::Success { result: true }.into_response()?)
}

fn endpoint_error(
    error: HashMap<String, String>,
    data: Arc<Data>,
    auth: AuthDetails,
) -> Fallible<Response<Body>> {
    let chunk = ExperimentChunk::run_by(&data.db, &Assignee::Agent(auth.name.clone()))?
        .ok_or_else(|| err_msg("no experiment chunk run by this agent"))?;

    let mut ex = Experiment::get(&data.db, &chunk.parent_name)?
        .ok_or_else(|| err_msg("no experiment with this name"))?;

    ex.set_status(&data.db, Status::Failed)?;

    if let Some(ref github_issue) = ex.github_issue {
        Message::new()
            .line(
                "rotating_light",
                format!(
                    "Experiment **`{}`** has encountered an error: {}",
                    ex.name,
                    error.get("error").unwrap_or(&String::from("no error")),
                ),
            )
            .line(
                "hammer_and_wrench",
                "If the error is fixed use the `retry` command.",
            )
            .note(
                "sos",
                "Can someone from the infra team check in on this? @rust-lang/infra",
            )
            .send(&github_issue.api_url, &data)?;
    }
    Ok(ApiResponse::Success { result: true }.into_response()?)
}

fn handle_results(resp: Fallible<Response<Body>>) -> Response<Body> {
    match resp {
        Ok(resp) => resp,
        Err(err) => ApiResponse::internal_error(err.to_string())
            .into_response()
            .unwrap(),
    }
}

fn handle_errors(err: Rejection) -> Result<Response<Body>, Rejection> {
    let error = if let Some(compat) = err.find_cause::<Compat<HttpError>>() {
        Some(*compat.get_ref())
    } else if let StatusCode::NOT_FOUND = err.status() {
        Some(HttpError::NotFound)
    } else if let StatusCode::METHOD_NOT_ALLOWED = err.status() {
        Some(HttpError::NotFound)
    } else {
        None
    };

    match error {
        Some(HttpError::NotFound) => Ok(ApiResponse::not_found().into_response().unwrap()),
        Some(HttpError::Forbidden) => Ok(ApiResponse::unauthorized().into_response().unwrap()),
        None => Err(err),
    }
}
