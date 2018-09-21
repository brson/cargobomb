use config::Config;
use crates::{self, Crate};
use errors::*;
use ex;
use ex_run;
use experiments::Experiment;
use git;
use results::{TestResult, WriteResults};
use std::fmt;
use toolchain::{Toolchain, MAIN_TOOLCHAIN};
use util;

pub enum TaskStep {
    Prepare,
    BuildAndTest { tc: Toolchain, quiet: bool },
    BuildOnly { tc: Toolchain, quiet: bool },
    CheckOnly { tc: Toolchain, quiet: bool },
    UnstableFeatures { tc: Toolchain },
}

impl fmt::Debug for TaskStep {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TaskStep::Prepare => write!(f, "prepare")?,
            TaskStep::BuildAndTest { ref tc, quiet } => {
                write!(f, "build and test {}", tc.to_string())?;
                if quiet {
                    write!(f, " (quiet)")?;
                }
            }
            TaskStep::BuildOnly { ref tc, quiet } => {
                write!(f, "build {}", tc.to_string())?;
                if quiet {
                    write!(f, " (quiet)")?;
                }
            }
            TaskStep::CheckOnly { ref tc, quiet } => {
                write!(f, "check {}", tc.to_string())?;
                if quiet {
                    write!(f, " (quiet)")?;
                }
            }
            TaskStep::UnstableFeatures { ref tc } => {
                write!(f, "find unstable features on {}", tc.to_string())?;
            }
        }
        Ok(())
    }
}

pub struct Task {
    pub krate: Crate,
    pub step: TaskStep,
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?} of crate {}", self.step, self.krate)
    }
}

impl Task {
    pub fn needs_exec<DB: WriteResults>(&self, ex: &Experiment, db: &DB) -> bool {
        // If an error happens while checking if the task should be executed, the error is ignored
        // and the function returns true.
        match self.step {
            // The prepare step should always be executed.
            // It will not be executed if all the dependent tasks are already executed, since the
            // runner will not reach the prepare task in that case.
            TaskStep::Prepare => true,
            // Build tasks should only be executed if there are no results for them
            TaskStep::BuildAndTest { ref tc, .. }
            | TaskStep::BuildOnly { ref tc, .. }
            | TaskStep::CheckOnly { ref tc, .. }
            | TaskStep::UnstableFeatures { ref tc } => {
                db.get_result(ex, tc, &self.krate).unwrap_or(None).is_none()
            }
        }
    }

    pub fn mark_as_failed<DB: WriteResults>(
        &self,
        ex: &Experiment,
        db: &DB,
        err: &Error,
        result: TestResult,
    ) -> Result<()> {
        match self.step {
            TaskStep::Prepare => {}
            TaskStep::BuildAndTest { ref tc, .. }
            | TaskStep::BuildOnly { ref tc, .. }
            | TaskStep::CheckOnly { ref tc, .. }
            | TaskStep::UnstableFeatures { ref tc } => {
                db.record_result(ex, tc, &self.krate, || {
                    error!("this task or one of its parent failed!");
                    util::report_error(err);
                    Ok(result)
                })?;
            }
        }

        Ok(())
    }

    pub fn run<DB: WriteResults>(&self, config: &Config, ex: &Experiment, db: &DB) -> Result<()> {
        match self.step {
            TaskStep::Prepare => self.run_prepare(config, ex, db),
            TaskStep::BuildAndTest { ref tc, quiet } => {
                self.run_build_and_test(config, ex, tc, db, quiet)
            }
            TaskStep::BuildOnly { ref tc, quiet } => self.run_build_only(config, ex, tc, db, quiet),
            TaskStep::CheckOnly { ref tc, quiet } => self.run_check_only(config, ex, tc, db, quiet),
            TaskStep::UnstableFeatures { ref tc } => self.run_unstable_features(config, ex, db, tc),
        }
    }

    fn run_prepare<DB: WriteResults>(
        &self,
        config: &Config,
        ex: &Experiment,
        db: &DB,
    ) -> Result<()> {
        // Fetch repository data if it's a git repo
        if let Some(repo) = self.krate.github() {
            if let Err(e) = git::shallow_clone_or_pull(&repo.url(), &repo.mirror_dir()) {
                util::report_error(&e);
            }

            ex::capture_shas(ex, &[self.krate.clone()], db)?;
        }

        crates::prepare_crate(&self.krate)?;
        ex::frob_toml(ex, &self.krate)?;
        ex::capture_lockfile(config, ex, &self.krate, &MAIN_TOOLCHAIN)?;
        ex::fetch_crate_deps(config, ex, &self.krate, &MAIN_TOOLCHAIN)?;

        Ok(())
    }

    fn run_build_and_test<DB: WriteResults>(
        &self,
        config: &Config,
        ex: &Experiment,
        tc: &Toolchain,
        db: &DB,
        quiet: bool,
    ) -> Result<()> {
        ex_run::run_test(
            config,
            "testing",
            ex,
            tc,
            &self.krate,
            db,
            quiet,
            ex_run::test_build_and_test,
        ).map(|_| ())
    }

    fn run_build_only<DB: WriteResults>(
        &self,
        config: &Config,
        ex: &Experiment,
        tc: &Toolchain,
        db: &DB,
        quiet: bool,
    ) -> Result<()> {
        ex_run::run_test(
            config,
            "testing",
            ex,
            tc,
            &self.krate,
            db,
            quiet,
            ex_run::test_build_only,
        ).map(|_| ())
    }

    fn run_check_only<DB: WriteResults>(
        &self,
        config: &Config,
        ex: &Experiment,
        tc: &Toolchain,
        db: &DB,
        quiet: bool,
    ) -> Result<()> {
        ex_run::run_test(
            config,
            "checking",
            ex,
            tc,
            &self.krate,
            db,
            quiet,
            ex_run::test_check_only,
        ).map(|_| ())
    }

    fn run_unstable_features<DB: WriteResults>(
        &self,
        config: &Config,
        ex: &Experiment,
        db: &DB,
        tc: &Toolchain,
    ) -> Result<()> {
        ex_run::run_test(
            config,
            "checking",
            ex,
            tc,
            &self.krate,
            db,
            false,
            ex_run::test_find_unstable_features,
        ).map(|_| ())
    }
}
