use crate::experiments::{Experiment, Mode, Status};
use crate::prelude::*;
use crate::server::routes::ui::{render_template, LayoutContext};
use crate::server::{Data, HttpError};
use chrono::{Duration, SecondsFormat, Utc};
use chrono_humanize::{Accuracy, HumanTime, Tense};
use http::Response;
use hyper::Body;
use std::sync::Arc;

#[derive(Serialize)]
struct ExperimentData {
    name: String,
    status_class: &'static str,
    status_pretty: &'static str,
    mode: &'static str,
    assigned_to: Option<String>,
    progress: u8,
    priority: i32,
}

impl ExperimentData {
    fn new(data: &Data, experiment: &Experiment) -> Fallible<Self> {
        let (status_class, status_pretty, show_progress) = match experiment.status {
            Status::Queued => ("", "Queued", true),
            Status::Running => ("orange", "Running", true),
            Status::NeedsReport => ("orange", "Needs report", false),
            Status::Failed => ("red", "Failed", false),
            Status::GeneratingReport => ("orange", "Generating report", false),
            Status::ReportFailed => ("red", "Report failed", false),
            Status::Completed => ("green", "Completed", false),
        };

        Ok(ExperimentData {
            name: experiment.name.clone(),
            status_class,
            status_pretty,
            mode: match experiment.mode {
                Mode::BuildAndTest => "cargo test",
                Mode::BuildOnly => "cargo build",
                Mode::CheckOnly => "cargo check",
                Mode::Clippy => "cargo clippy",
                Mode::Rustdoc => "cargo doc",
                Mode::UnstableFeatures => "unstable features",
            },
            assigned_to: experiment.assigned_to.as_ref().map(|a| a.to_string()),
            priority: experiment.priority,
            progress: if show_progress {
                experiment.progress(&data.db)?
            } else {
                100
            },
        })
    }
}

#[derive(Serialize)]
struct ListContext {
    layout: LayoutContext,
    experiments: Vec<ExperimentData>,
}

pub fn endpoint_queue(data: Arc<Data>) -> Fallible<Response<Body>> {
    let mut queued = Vec::new();
    let mut running = Vec::new();
    let mut needs_report = Vec::new();
    let mut failed = Vec::new();
    let mut generating_report = Vec::new();
    let mut report_failed = Vec::new();

    for experiment in Experiment::unfinished(&data.db)? {
        // Don't include completed experiments in the queue
        if experiment.status == Status::Completed {
            continue;
        }

        let ex = ExperimentData::new(&data, &experiment)?;

        match experiment.status {
            Status::Queued => queued.push(ex),
            Status::Running => running.push(ex),
            Status::NeedsReport => needs_report.push(ex),
            Status::Failed => failed.push(ex),
            Status::GeneratingReport => generating_report.push(ex),
            Status::ReportFailed => report_failed.push(ex),
            Status::Completed => unreachable!(),
        };
    }

    let mut experiments = Vec::new();
    experiments.append(&mut report_failed);
    experiments.append(&mut generating_report);
    experiments.append(&mut needs_report);
    experiments.append(&mut failed);
    experiments.append(&mut running);
    experiments.append(&mut queued);

    render_template(
        "ui/queue.html",
        &ListContext {
            layout: LayoutContext::new(),
            experiments,
        },
    )
}

#[derive(Serialize)]
struct ExperimentExt {
    #[serde(flatten)]
    common: ExperimentData,

    github_url: Option<String>,
    report_url: Option<String>,

    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,

    total_jobs: u32,
    completed_jobs: u32,
    duration: Option<String>,
    estimated_end: Option<String>,
    average_job_duration: Option<String>,
}

#[derive(Serialize)]
struct ExperimentContext {
    experiment: ExperimentExt,
    layout: LayoutContext,
}

pub fn endpoint_experiment(name: String, data: Arc<Data>) -> Fallible<Response<Body>> {
    if let Some(ex) = Experiment::get(&data.db, &name)? {
        let (completed_jobs, total_jobs) = ex.raw_progress(&data.db)?;

        let (duration, estimated_end, average_job_duration) = if completed_jobs > 0
            && total_jobs > 0
        {
            if let Some(started_at) = ex.started_at {
                let res = if let Some(completed_at) = ex.completed_at {
                    let total = completed_at.signed_duration_since(started_at);
                    (
                        Some(total),
                        None,
                        Some((total / completed_jobs as i32).num_seconds()),
                    )
                } else {
                    let total = Utc::now().signed_duration_since(started_at);
                    let job_duration = total / completed_jobs as i32;
                    (
                        None,
                        Some(job_duration * (total_jobs as i32 - completed_jobs as i32)),
                        Some(job_duration.num_seconds()),
                    )
                };

                (
                    res.0
                        .map(|r| HumanTime::from(r).to_text_en(Accuracy::Rough, Tense::Present)),
                    res.1
                        .map(|r| HumanTime::from(r).to_text_en(Accuracy::Rough, Tense::Present)),
                    res.2.map(|r| {
                        HumanTime::from(Duration::seconds(r))
                            .to_text_en(Accuracy::Precise, Tense::Present)
                    }),
                )
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        };

        let experiment = ExperimentExt {
            common: ExperimentData::new(&data, &ex)?,

            github_url: ex.github_issue.map(|i| i.html_url.clone()),
            report_url: ex.report_url.clone(),

            created_at: ex.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            started_at: ex
                .started_at
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)),
            completed_at: ex
                .completed_at
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)),

            total_jobs,
            completed_jobs,
            duration,
            estimated_end,
            average_job_duration,
        };

        render_template(
            "ui/experiment.html",
            &ExperimentContext {
                layout: LayoutContext::new(),
                experiment,
            },
        )
    } else {
        Err(HttpError::NotFound.into())
    }
}
