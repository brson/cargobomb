/*!

Cargobomb works by serially processing a queue of commands, each of
which transforms the application state in some discrete way, and
designed to be resilient to I/O errors. The application state is
backed by a directory in the filesystem, and optionally synchronized
with s3.

These command queues may be created dynamically and executed in
parallel jobs, either locally, or distributed on e.g. AWS. The
application state employs ownership techniques to ensure that
parallel access is consistent and race-free.

NB: The design of this module is SERIOUSLY MESSED UP, with lots of
duplication, the result of a deep yak shave that failed. It needs a
rewrite.

*/

#![allow(dead_code)]

use docker;
use errors::*;
use ex;
use ex_run;
use job::{self, JobId};
use lists;
use report;
use run;
use toolchain::Toolchain;

// An experiment name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ex(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SayMsg(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job(pub JobId);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Cmd {
    /* Basic synchronous commands */

    // Local prep
    PrepareLocal,
    PrepareToolchain(Toolchain),
    BuildContainer,

    // List creation
    CreateLists,
    CreateListsFull,
    CreateRecentList,
    CreateHotList,
    CreatePopList,
    CreateGhCandidateList,
    CreateGhAppList,
    CreateGhCandidateListFromCache,
    CreateGhAppListFromCache,

    // Master experiment prep
    DefineEx(Ex, Toolchain, Toolchain, ExMode, ExCrateSelect),
    PrepareEx(Ex),
    CopyEx(Ex, Ex),
    DeleteEx(Ex),

    // Shared experiment prep
    PrepareExShared(Ex),
    FetchGhMirrors(Ex),
    CaptureShas(Ex),
    DownloadCrates(Ex),
    FrobCargoTomls(Ex),
    CaptureLockfiles(Ex, Toolchain),

    // Local experiment prep
    PrepareExLocal(Ex),
    DeleteAllTargetDirs(Ex),
    DeleteAllResults(Ex),
    FetchDeps(Ex, Toolchain),
    PrepareAllToolchains(Ex),

    // Experimenting
    Run(Ex),
    RunTc(Ex, Toolchain),

    // Reporting
    GenReport(Ex),

    // Job control
    CreateDockerJob(Box<Cmd>),
    StartJob(Job),
    WaitForJob(Job),
    RunJob(Job),
    RunJobAgain(Job),
    RunCmdForJob(Job),

    // Misc
    Sleep,
    Say(SayMsg),
}

trait NewCmd {
    fn name(&self) -> &'static str;

    fn process(&self) -> Result<()> {
        Ok(())
    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![])
    }
}

struct PrepareLocal;

impl NewCmd for PrepareLocal {
    fn name(&self) -> &'static str {
        "prepare-local"
    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(PrepareToolchain("stable".parse()?)),
            Box::new(BuildContainer),
            Box::new(CreateLists),
        ])
    }
}

struct PrepareToolchain(Toolchain);

impl NewCmd for PrepareToolchain {
    fn name(&self) -> &'static str {
        "prepare-toolchain"
    }

    fn process(&self) -> Result<()> {
        self.0.prepare()
    }
}

struct BuildContainer;

impl NewCmd for BuildContainer {
    fn name(&self) -> &'static str {
        "build-container"

    }

    fn process(&self) -> Result<()> {
        docker::build_container()
    }
}

struct CreateLists;

impl NewCmd for CreateLists {
    fn name(&self) -> &'static str {
        "create-lists"

    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(CreateRecentList),
            Box::new(CreateHotList),
            Box::new(CreatePopList),
            Box::new(CreateGhCandidateListFromCache),
            Box::new(CreateGhAppListFromCache),
        ])
    }
}

struct CreateListsFull;

impl NewCmd for CreateListsFull {
    fn name(&self) -> &'static str {
        "create-lists-full"

    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(CreateRecentList),
            Box::new(CreateHotList),
            Box::new(CreatePopList),
            Box::new(CreateGhCandidateList),
            Box::new(CreateGhAppList),
        ])
    }
}

struct CreateRecentList;

impl NewCmd for CreateRecentList {
    fn name(&self) -> &'static str {
        "create-recent-list"

    }

    fn process(&self) -> Result<()> {
        lists::create_recent_list()
    }
}

struct CreateHotList;

impl NewCmd for CreateHotList {
    fn name(&self) -> &'static str {
        "create-hot-list"

    }

    fn process(&self) -> Result<()> {
        lists::create_hot_list()
    }
}

struct CreatePopList;

impl NewCmd for CreatePopList {
    fn name(&self) -> &'static str {
        "create-pop-list"

    }

    fn process(&self) -> Result<()> {
        lists::create_pop_list()
    }
}

struct CreateGhCandidateList;

impl NewCmd for CreateGhCandidateList {
    fn name(&self) -> &'static str {
        "create-gh-candidate-list"

    }

    fn process(&self) -> Result<()> {
        lists::create_gh_candidate_list()
    }
}

struct CreateGhAppList;

impl NewCmd for CreateGhAppList {
    fn name(&self) -> &'static str {
        "create-gh-app-list"

    }

    fn process(&self) -> Result<()> {
        lists::create_gh_app_list()
    }
}

struct CreateGhCandidateListFromCache;

impl NewCmd for CreateGhCandidateListFromCache {
    fn name(&self) -> &'static str {
        "create-gh-candidate-list-from-cache"

    }

    fn process(&self) -> Result<()> {
        lists::create_gh_candidate_list_from_cache()
    }
}

struct CreateGhAppListFromCache;

impl NewCmd for CreateGhAppListFromCache {
    fn name(&self) -> &'static str {
        "create-gh-app-list-from-cache"

    }

    fn process(&self) -> Result<()> {
        lists::create_gh_candidate_list_from_cache()
    }
}

struct DefineEx(Ex, Toolchain, Toolchain, ExMode, ExCrateSelect);

impl NewCmd for DefineEx {
    fn name(&self) -> &'static str {
        "define-ex"

    }

    fn process(&self) -> Result<()> {
        ex::define(ex::ExOpts {
                       name: (self.0).0.clone(),
                       toolchains: vec![self.1.clone(), self.2.clone()],
                       mode: self.3.clone(),
                       crates: self.4.clone(),
                   })
    }
}

struct PrepareEx(Ex);

impl NewCmd for PrepareEx {
    fn name(&self) -> &'static str {
        "prepare-ex"

    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(PrepareExShared(self.0.clone())),
            Box::new(PrepareExLocal(self.0.clone())),
        ])
    }
}

struct CopyEx(Ex, Ex);

impl NewCmd for CopyEx {
    fn name(&self) -> &'static str {

        "copy-ex"
    }

    fn process(&self) -> Result<()> {
        ex::copy(&(self.0).0, &(self.1).0)
    }
}

struct DeleteEx(Ex);

impl NewCmd for DeleteEx {
    fn name(&self) -> &'static str {
        "delete-ex"

    }

    fn process(&self) -> Result<()> {
        ex::delete(&(self.0).0)
    }
}

struct PrepareExShared(Ex);

impl NewCmd for PrepareExShared {
    fn name(&self) -> &'static str {
        "prepare-ex-shared"

    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(FetchGhMirrors(self.0.clone())),
            Box::new(CaptureShas(self.0.clone())),
            Box::new(DownloadCrates(self.0.clone())),
            Box::new(FrobCargoTomls(self.0.clone())),
            Box::new(CaptureLockfiles(self.0.clone(), "stable".parse()?)),
        ])
    }
}

struct FetchGhMirrors(Ex);

impl NewCmd for FetchGhMirrors {
    fn name(&self) -> &'static str {

        "fetch-gh-mirrors"
    }

    fn process(&self) -> Result<()> {
        ex::fetch_gh_mirrors(&(self.0).0)
    }
}

struct CaptureShas(Ex);

impl NewCmd for CaptureShas {
    fn name(&self) -> &'static str {
        "capture-shas"

    }

    fn process(&self) -> Result<()> {
        ex::capture_shas(&(self.0).0)
    }
}

struct DownloadCrates(Ex);

impl NewCmd for DownloadCrates {
    fn name(&self) -> &'static str {

        "download-crates"
    }

    fn process(&self) -> Result<()> {
        ex::download_crates(&(self.0).0)
    }
}

struct FrobCargoTomls(Ex);

impl NewCmd for FrobCargoTomls {
    fn name(&self) -> &'static str {

        "frob-cargo-tomls"
    }

    fn process(&self) -> Result<()> {
        ex::frob_tomls(&(self.0).0)
    }
}

struct CaptureLockfiles(Ex, Toolchain);

impl NewCmd for CaptureLockfiles {
    fn name(&self) -> &'static str {
        "capture-lockfiles"

    }

    fn process(&self) -> Result<()> {
        ex::capture_lockfiles(&(self.0).0, &self.1, false)
    }
}

struct PrepareExLocal(Ex);

impl NewCmd for PrepareExLocal {
    fn name(&self) -> &'static str {
        "prepare-ex-local"

    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(DeleteAllTargetDirs(self.0.clone())),
            Box::new(DeleteAllResults(self.0.clone())),
            Box::new(FetchDeps(self.0.clone(), "stable".parse()?)),
            Box::new(PrepareAllToolchains(self.0.clone())),
        ])
    }
}

struct DeleteAllTargetDirs(Ex);

impl NewCmd for DeleteAllTargetDirs {
    fn name(&self) -> &'static str {
        "delete-all-target-dirs"

    }

    fn process(&self) -> Result<()> {
        ex::delete_all_target_dirs(&(self.0).0)
    }
}

struct DeleteAllResults(Ex);

impl NewCmd for DeleteAllResults {
    fn name(&self) -> &'static str {
        "delete-all-results"

    }

    fn process(&self) -> Result<()> {
        ex_run::delete_all_results(&(self.0).0)
    }
}

struct FetchDeps(Ex, Toolchain);

impl NewCmd for FetchDeps {
    fn name(&self) -> &'static str {
        "fetch-deps"

    }

    fn process(&self) -> Result<()> {
        ex::fetch_deps(&(self.0).0, &self.1)
    }
}

struct PrepareAllToolchains(Ex);

impl NewCmd for PrepareAllToolchains {
    fn name(&self) -> &'static str {
        "prepare-all-toolchains"
    }

    fn sub_cmds(&self) -> Result<Vec<Box<NewCmd>>> {
        Ok(vec![
            Box::new(DeleteAllTargetDirs(self.0.clone())),
            Box::new(DeleteAllResults(self.0.clone())),
            Box::new(FetchDeps(self.0.clone(), "stable".parse()?)),
            Box::new(PrepareAllToolchains(self.0.clone())),
        ])
    }
}

struct Run(Ex);

impl NewCmd for Run {
    fn name(&self) -> &'static str {
        "run"
    }

    fn process(&self) -> Result<()> {
        ex_run::run_ex_all_tcs(&(self.0).0)
    }
}

struct RunTc(Ex, Toolchain);

impl NewCmd for RunTc {
    fn name(&self) -> &'static str {
        "run-tc"
    }

    fn process(&self) -> Result<()> {
        ex_run::run_ex(&(self.0).0, self.1.clone())
    }
}

struct GenReport(Ex);

impl NewCmd for GenReport {
    fn name(&self) -> &'static str {
        "gen-report"
    }

    fn process(&self) -> Result<()> {
        report::gen(&(self.0).0)
    }
}

struct CreateDockerJob(Box<Cmd>);

impl NewCmd for CreateDockerJob {
    fn name(&self) -> &'static str {
        "create-docker-job"
    }

    fn process(&self) -> Result<()> {
        job::create_local(*(self.0.clone()))
    }
}

struct StartJob(Job);

impl NewCmd for StartJob {
    fn name(&self) -> &'static str {
        "start-job"
    }

    fn process(&self) -> Result<()> {
        job::start((self.0).0)
    }
}

struct WaitForJob(Job);

impl NewCmd for WaitForJob {
    fn name(&self) -> &'static str {
        "wait-for-job"
    }

    fn process(&self) -> Result<()> {
        job::wait((self.0).0)
    }
}

struct RunJob(Job);

impl NewCmd for RunJob {
    fn name(&self) -> &'static str {
        "run-job"
    }

    fn process(&self) -> Result<()> {
        job::run((self.0).0)
    }
}

struct RunJobAgain(Job);

impl NewCmd for RunJobAgain {
    fn name(&self) -> &'static str {
        "run-job-again"
    }

    fn process(&self) -> Result<()> {
        job::run_again((self.0).0)
    }
}

struct RunCmdForJob(Job);

impl NewCmd for RunCmdForJob {
    fn name(&self) -> &'static str {

        "run-cmd-for-job"
    }

    fn process(&self) -> Result<()> {
        job::run_cmd_for_job((self.0).0)
    }
}

struct Sleep;

impl NewCmd for Sleep {
    fn name(&self) -> &'static str {

        "sleep"
    }

    fn process(&self) -> Result<()> {
        run::run("sleep", &["5"], &[])
    }
}

struct Say(SayMsg);

impl NewCmd for Say {
    fn name(&self) -> &'static str {

        "say"
    }

    fn process(&self) -> Result<()> {
        log!("{}", (self.0).0);
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[derive(Debug, Clone)]
pub enum ExMode {
    BuildAndTest,
    BuildOnly,
    CheckOnly,
    UnstableFeatures,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ExCrateSelect {
    Full,
    Demo,
    SmallRandom,
    Top100,
}

use bmk::Process;

impl Process for Cmd {
    fn process(self) -> Result<Vec<Cmd>> {
        use lists;
        use docker;
        use ex;
        use ex_run;
        use run;
        use report;
        use job;

        let mut cmds = Vec::new();
        match self {
            // Local prep
            Cmd::PrepareLocal => {
                cmds.extend(vec![
                    Cmd::PrepareToolchain("stable".parse()?),
                    Cmd::BuildContainer,
                    Cmd::CreateLists,
                ]);
            }
            Cmd::PrepareToolchain(tc) => tc.prepare()?,
            Cmd::BuildContainer => docker::build_container()?,

            // List creation
            Cmd::CreateLists => {
                cmds.extend(vec![
                    Cmd::CreateRecentList,
                    Cmd::CreateHotList,
                    Cmd::CreatePopList,
                    Cmd::CreateGhCandidateListFromCache,
                    Cmd::CreateGhAppListFromCache,
                ]);
            }
            Cmd::CreateListsFull => {
                cmds.extend(vec![
                    Cmd::CreateRecentList,
                    Cmd::CreateHotList,
                    Cmd::CreatePopList,
                    Cmd::CreateGhCandidateList,
                    Cmd::CreateGhAppList,
                ]);
            }
            Cmd::CreateRecentList => lists::create_recent_list()?,
            Cmd::CreateHotList => lists::create_hot_list()?,
            Cmd::CreatePopList => lists::create_pop_list()?,
            Cmd::CreateGhCandidateList => lists::create_gh_candidate_list()?,
            Cmd::CreateGhAppList => lists::create_gh_app_list()?,
            Cmd::CreateGhCandidateListFromCache => lists::create_gh_candidate_list_from_cache()?,
            Cmd::CreateGhAppListFromCache => lists::create_gh_app_list_from_cache()?,

            // Experiment prep
            Cmd::DefineEx(ex, tc1, tc2, mode, crates) => {
                ex::define(ex::ExOpts {
                               name: ex.0,
                               toolchains: vec![tc1, tc2],
                               mode: mode,
                               crates: crates,
                           })?;
            }
            Cmd::PrepareEx(ex) => {
                cmds.extend(vec![Cmd::PrepareExShared(ex.clone()), Cmd::PrepareExLocal(ex)]);
            }
            Cmd::CopyEx(ex1, ex2) => ex::copy(&ex1.0, &ex2.0)?,
            Cmd::DeleteEx(ex) => ex::delete(&ex.0)?,

            // Shared emperiment prep
            Cmd::PrepareExShared(ex) => {
                cmds.extend(vec![
                    Cmd::FetchGhMirrors(ex.clone()),
                    Cmd::CaptureShas(ex.clone()),
                    Cmd::DownloadCrates(ex.clone()),
                    Cmd::FrobCargoTomls(ex.clone()),
                    Cmd::CaptureLockfiles(ex, "stable".parse()?),
                ]);
            }
            Cmd::FetchGhMirrors(ex) => ex::fetch_gh_mirrors(&ex.0)?,
            Cmd::CaptureShas(ex) => ex::capture_shas(&ex.0)?,
            Cmd::DownloadCrates(ex) => ex::download_crates(&ex.0)?,
            Cmd::FrobCargoTomls(ex) => ex::frob_tomls(&ex.0)?,
            Cmd::CaptureLockfiles(ex, tc) => ex::capture_lockfiles(&ex.0, &tc, false)?,

            // Local experiment prep
            Cmd::PrepareExLocal(ex) => {
                cmds.extend(vec![
                    Cmd::DeleteAllTargetDirs(ex.clone()),
                    Cmd::DeleteAllResults(ex.clone()),
                    Cmd::FetchDeps(ex.clone(), "stable".parse()?),
                    Cmd::PrepareAllToolchains(ex),
                ]);
            }
            Cmd::DeleteAllTargetDirs(ex) => ex::delete_all_target_dirs(&ex.0)?,
            Cmd::DeleteAllResults(ex) => ex_run::delete_all_results(&ex.0)?,
            Cmd::FetchDeps(ex, tc) => ex::fetch_deps(&ex.0, &tc)?,
            Cmd::PrepareAllToolchains(ex) => ex::prepare_all_toolchains(&ex.0)?,

            // Experimenting
            Cmd::Run(ex) => ex_run::run_ex_all_tcs(&ex.0)?,
            Cmd::RunTc(ex, tc) => ex_run::run_ex(&ex.0, tc)?,

            // Reporting
            Cmd::GenReport(ex) => report::gen(&ex.0)?,

            // Job control
            Cmd::CreateDockerJob(cmd) => job::create_local(*cmd)?,
            Cmd::StartJob(job) => job::start(job.0)?,
            Cmd::WaitForJob(job) => job::wait(job.0)?,
            Cmd::RunJob(job) => job::run(job.0)?,
            Cmd::RunJobAgain(job) => job::run_again(job.0)?,
            Cmd::RunCmdForJob(job) => job::run_cmd_for_job(job.0)?,

            // Misc
            Cmd::Sleep => run::run("sleep", &["5"], &[])?,
            Cmd::Say(msg) => log!("{}", msg.0),
        }

        Ok(cmds)
    }
}

// Boilerplate conversions on the model. Ideally all this would be generated.
pub mod conv {
    use super::*;

    use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
    use std::str::FromStr;

    pub fn clap_cmds() -> Vec<App<'static, 'static>> {
        clap_cmds_(true)
    }

    fn clap_cmds_(recurse: bool) -> Vec<App<'static, 'static>> {
        // Types of arguments
        let ex = || opt("ex", "default");
        let ex1 = || req("ex-1");
        let ex2 = || req("ex-2");
        let req_tc = || req("tc");
        let opt_tc = || opt("tc", "stable");
        let tc1 = || req("tc-1");
        let tc2 = || req("tc-2");
        let mode = || {
            Arg::with_name("mode")
                .required(false)
                .long("mode")
                .default_value(ExMode::BuildAndTest.to_str())
                .possible_values(&[
                    ExMode::BuildAndTest.to_str(),
                    ExMode::BuildOnly.to_str(),
                    ExMode::CheckOnly.to_str(),
                    ExMode::UnstableFeatures.to_str(),
                ])
        };
        let crate_select = || {
            Arg::with_name("crate-select")
                .required(false)
                .long("crate-select")
                .default_value(ExCrateSelect::Demo.to_str())
                .possible_values(&[
                    ExCrateSelect::Demo.to_str(),
                    ExCrateSelect::Full.to_str(),
                    ExCrateSelect::SmallRandom.to_str(),
                    ExCrateSelect::Top100.to_str(),
                ])
        };
        let job = || req("job");
        let say_msg = || req("say-msg");

        fn opt(n: &'static str, def: &'static str) -> Arg<'static, 'static> {
            Arg::with_name(n)
                .required(false)
                .long(n)
                .default_value(def)
        }

        fn req(n: &'static str) -> Arg<'static, 'static> {
            Arg::with_name(n).required(true)
        }

        fn cmd(n: &'static str, desc: &'static str) -> App<'static, 'static> {
            SubCommand::with_name(n).about(desc)
        }

        vec![
            // Local prep
            cmd("prepare-local",
                "acquire toolchains, build containers, build crate lists"),
            cmd("prepare-toolchain", "install or update a toolchain").arg(req_tc()),
            cmd("build-container",
                "build docker container needed by experiments"),

            // List creation
            cmd("create-lists", "create all the lists of crates"),
            cmd("create-lists-full", "create all the lists of crates"),
            cmd("create-recent-list",
                "create the list of most recent crate versions"),
            cmd("create-hot-list",
                "create the list of popular crates versions"),
            cmd("create-pop-list", "create the list of popular crates"),
            cmd("create-gh-candidate-list",
                "crate the list of all GitHub Rust repos"),
            cmd("create-gh-app-list",
                "create the list of GitHub Rust applications"),
            cmd("create-gh-candidate-list-from-cache",
                "crate the list of all GitHub Rust repos from cache"),
            cmd("create-gh-app-list-from-cache",
                "create the list of GitHub Rust applications from cache"),

            // Master experiment prep
            cmd("define-ex", "define an experiment")
                .arg(ex())
                .arg(tc1())
                .arg(tc2())
                .arg(mode())
                .arg(crate_select()),
            cmd("prepare-ex", "prepare shared and local data for experiment").arg(ex()),
            cmd("copy-ex", "copy all data from one experiment to another")
                .arg(ex1())
                .arg(ex2()),
            cmd("delete-ex", "delete shared data for experiment").arg(ex()),

            // Shared experiment prep
            cmd("prepare-ex-shared", "prepare shared data for experiment").arg(ex()),
            cmd("fetch-gh-mirrors", "fetch github repos for experiment").arg(ex()),
            cmd("capture-shas", "record the head commits of GitHub repos").arg(ex()),
            cmd("download-crates", "download crates to local disk").arg(ex()),
            cmd("frob-cargo-tomls", "frobsm tomls for experiment crates").arg(ex()),
            cmd("capture-lockfiles",
                "records lockfiles for all crates in experiment")
                    .arg(ex())
                    .arg(opt_tc()),

            // Local experiment prep
            cmd("prepare-ex-local", "prepare local data for experiment").arg(ex()),
            cmd("delete-all-target-dirs",
                "delete the cargo target dirs for an experiment")
                    .arg(ex()),
            cmd("delete-all-results", "delete all results for an experiment").arg(ex()),
            cmd("fetch-deps", "fetch all dependencies for an experiment")
                .arg(ex())
                .arg(opt_tc()),
            cmd("prepare-all-toolchains",
                "prepare all toolchains for local experiment")
                    .arg(ex()),

            // Experimenting
            cmd("run", "run an experiment, with all toolchains").arg(ex()),
            cmd("run-tc", "run an experiment, with a single toolchain")
                .arg(ex())
                .arg(req_tc()),

            // Reporting
            cmd("gen-report", "generate the experiment report").arg(ex()),

            // Job control
            if recurse {
                cmd("create-docker-job", "start a docker job in docker")
                    .subcommands(clap_cmds_(false))
            } else {
                cmd("create-docker-job", "nop")
            },
            cmd("start-job", "start a job asynchronously").arg(job()),
            cmd("wait-for-job", "wait for a job to complete").arg(job()),
            cmd("run-job", "run a pending job synchronously").arg(job()),
            cmd("run-job-again", "run a completed job again synchronously").arg(job()),
            cmd("run-cmd-for-job",
                "run a command for a job, inside the job environment")
                    .arg(job()),

            // Misc
            cmd("sleep", "sleep"),
            cmd("say", "say something").arg(say_msg()),
        ]
    }

    pub fn clap_args_to_cmd(m: &ArgMatches) -> Result<Cmd> {

        fn ex(m: &ArgMatches) -> Result<Ex> {
            m.value_of("ex").expect("").parse::<Ex>()
        }

        fn ex1(m: &ArgMatches) -> Result<Ex> {
            m.value_of("ex-1").expect("").parse::<Ex>()
        }

        fn ex2(m: &ArgMatches) -> Result<Ex> {
            m.value_of("ex-2").expect("").parse::<Ex>()
        }

        fn tc(m: &ArgMatches) -> Result<Toolchain> {
            m.value_of("tc").expect("").parse()
        }

        fn tc1(m: &ArgMatches) -> Result<Toolchain> {
            m.value_of("tc-1").expect("").parse()
        }

        fn tc2(m: &ArgMatches) -> Result<Toolchain> {
            m.value_of("tc-2").expect("").parse()
        }

        fn mode(m: &ArgMatches) -> Result<ExMode> {
            m.value_of("mode").expect("").parse::<ExMode>()
        }

        fn crate_select(m: &ArgMatches) -> Result<ExCrateSelect> {
            m.value_of("crate-select")
                .expect("")
                .parse::<ExCrateSelect>()
        }

        fn cmd(m: &ArgMatches) -> Result<Box<Cmd>> {
            Ok(Box::new(clap_args_to_cmd(m)?))
        }

        fn job(m: &ArgMatches) -> Result<Job> {
            m.value_of("job").expect("").parse::<Job>()
        }

        fn say_msg(m: &ArgMatches) -> Result<SayMsg> {
            Ok(SayMsg(m.value_of("say-msg").expect("").to_string()))
        }

        Ok(match m.subcommand() {
               // Local prep
               ("prepare-local", _) => Cmd::PrepareLocal,
               ("prepare-toolchain", Some(m)) => Cmd::PrepareToolchain(tc(m)?),
               ("build-container", _) => Cmd::BuildContainer,

               // List creation
               ("create-lists", _) => Cmd::CreateLists,
               ("create-lists-full", _) => Cmd::CreateListsFull,
               ("create-recent-list", _) => Cmd::CreateRecentList,
               ("create-hot-list", _) => Cmd::CreateHotList,
               ("create-pop-list", _) => Cmd::CreatePopList,
               ("create-gh-candidate-list", _) => Cmd::CreateGhCandidateList,
               ("create-gh-app-list", _) => Cmd::CreateGhAppList,
               ("create-gh-candidate-list-from-cache", _) => Cmd::CreateGhCandidateListFromCache,
               ("create-gh-app-list-from-cache", _) => Cmd::CreateGhAppListFromCache,

               // Master experiment prep
               ("define-ex", Some(m)) => {
                   Cmd::DefineEx(ex(m)?, tc1(m)?, tc2(m)?, mode(m)?, crate_select(m)?)
               }
               ("prepare-ex", Some(m)) => Cmd::PrepareEx(ex(m)?),
               ("copy-ex", Some(m)) => Cmd::CopyEx(ex1(m)?, ex2(m)?),
               ("delete-ex", Some(m)) => Cmd::DeleteEx(ex(m)?),

               // Shared experiment prep
               ("prepare-ex-shared", Some(m)) => Cmd::PrepareExShared(ex(m)?),
               ("fetch-gh-mirrors", Some(m)) => Cmd::FetchGhMirrors(ex(m)?),
               ("capture-shas", Some(m)) => Cmd::CaptureShas(ex(m)?),
               ("download-crates", Some(m)) => Cmd::DownloadCrates(ex(m)?),
               ("frob-cargo-tomls", Some(m)) => Cmd::FrobCargoTomls(ex(m)?),
               ("capture-lockfiles", Some(m)) => Cmd::CaptureLockfiles(ex(m)?, tc(m)?),

               // Local experiment prep
               ("prepare-ex-local", Some(m)) => Cmd::PrepareExLocal(ex(m)?),
               ("delete-all-target-dirs", Some(m)) => Cmd::DeleteAllTargetDirs(ex(m)?),
               ("delete-all-results", Some(m)) => Cmd::DeleteAllResults(ex(m)?),
               ("fetch-deps", Some(m)) => Cmd::FetchDeps(ex(m)?, tc(m)?),
               ("prepare-all-toolchains", Some(m)) => Cmd::PrepareAllToolchains(ex(m)?),

               // Experimenting
               ("run", Some(m)) => Cmd::Run(ex(m)?),
               ("run-tc", Some(m)) => Cmd::RunTc(ex(m)?, tc(m)?),

               // Reporting
               ("gen-report", Some(m)) => Cmd::GenReport(ex(m)?),

               // Job control
               ("create-docker-job", Some(m)) => Cmd::CreateDockerJob(cmd(m)?),
               ("start-job", Some(m)) => Cmd::StartJob(job(m)?),
               ("wait-for-job", Some(m)) => Cmd::WaitForJob(job(m)?),
               ("run-job", Some(m)) => Cmd::RunJob(job(m)?),
               ("run-job-again", Some(m)) => Cmd::RunJobAgain(job(m)?),
               ("run-cmd-for-job", Some(m)) => Cmd::RunCmdForJob(job(m)?),

               // Misc
               ("sleep", _) => Cmd::Sleep,
               ("say", Some(m)) => Cmd::Say(say_msg(m)?),

               (s, _) => panic!("unimplemented args_to_cmd {}", s),
           })
    }

    fn cmd_to_name(cmd: &Cmd) -> &'static str {
        use super::Cmd::*;
        match *cmd {
            PrepareLocal => "prepare-local",
            PrepareToolchain(..) => "prepare-toolchain",
            BuildContainer => "build-container",

            CreateLists => "create-lists",
            CreateListsFull => "create-lists-full",
            CreateRecentList => "create-recent-list",
            CreateHotList => "create-hot-list",
            CreatePopList => "create-pop-list",
            CreateGhCandidateList => "create-gh-candidate-list",
            CreateGhAppList => "create-gh-app-list",
            CreateGhCandidateListFromCache => "create-gh-candidate-list-from-cache",
            CreateGhAppListFromCache => "create-gh-app-list-from-cache",

            DefineEx(..) => "define-ex",
            PrepareEx(..) => "prepare-ex",
            CopyEx(..) => "copy-ex",
            DeleteEx(..) => "delete-ex",

            PrepareExShared(..) => "prepare-ex-shared",
            FetchGhMirrors(..) => "fetch-gh-mirrors",
            CaptureShas(..) => "capture-shas",
            DownloadCrates(..) => "download-crates",
            FrobCargoTomls(..) => "frob-cargo-tomls",
            CaptureLockfiles(..) => "capture-lockfiles",

            PrepareExLocal(..) => "prepare-ex-local",
            DeleteAllTargetDirs(..) => "delete-all-target-dirs",
            DeleteAllResults(..) => "delete-all-results",
            FetchDeps(..) => "fetch-deps",
            PrepareAllToolchains(..) => "prepare-all-toolchains",

            Run(..) => "run",
            RunTc(..) => "run-tc",

            GenReport(..) => "gen-report",

            CreateDockerJob(..) => "create-docker-job",
            StartJob(..) => "start-job",
            WaitForJob(..) => "wait-for-job",
            RunJob(..) => "run-job",
            RunJobAgain(..) => "run-job-again",
            RunCmdForJob(..) => "run-cmd-for-job",

            Sleep => "sleep",
            Say(..) => "say",
        }
    }

    pub fn cmd_to_args(cmd: Cmd) -> Vec<String> {
        Some(cmd_to_name(&cmd))
            .into_iter()
            .map(|s| s.to_string())
            .chain(cmd_to_args_(cmd).into_iter())
            .collect()
    }

    fn cmd_to_args_(cmd: Cmd) -> Vec<String> {
        #![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
        use super::Cmd::*;

        fn opt_ex(ex: Ex) -> String {
            format!("--ex={}", ex.0)
        }

        fn req_ex(ex: Ex) -> String {
            ex.0
        }

        fn opt_tc(tc: Toolchain) -> String {
            format!("--tc={}", tc.to_string())
        }

        fn req_tc(tc: Toolchain) -> String {
            tc.to_string()
        }

        fn opt_mode(mode: ExMode) -> String {
            format!("--mode={}", mode.to_str())
        }

        fn opt_crate_select(crate_select: ExCrateSelect) -> String {
            format!("--crate-select={}", crate_select.to_str())
        }

        fn req_job(job: Job) -> String {
            job.0.to_string()
        }

        fn req_say_msg(say_msg: SayMsg) -> String {
            say_msg.0
        }

        #[cfg_attr(feature = "cargo-clippy", allow(match_same_arms))]
        match cmd {
            PrepareLocal |
            BuildContainer |
            CreateLists |
            CreateListsFull |
            CreateRecentList |
            CreateHotList |
            CreatePopList |
            CreateGhCandidateList |
            CreateGhAppList |
            CreateGhCandidateListFromCache |
            CreateGhAppListFromCache |
            Sleep => vec![],

            PrepareToolchain(tc) => vec![req_tc(tc)],
            DefineEx(ex, tc1, tc2, mode, crate_select) => {
                vec![
                    opt_ex(ex),
                    req_tc(tc1),
                    req_tc(tc2),
                    opt_mode(mode),
                    opt_crate_select(crate_select),
                ]
            }
            PrepareEx(ex) => vec![opt_ex(ex)],
            CopyEx(ex1, ex2) => vec![req_ex(ex1), req_ex(ex2)],

            DeleteEx(ex) => vec![opt_ex(ex)],
            PrepareExShared(ex) => vec![opt_ex(ex)],
            FetchGhMirrors(ex) => vec![opt_ex(ex)],
            CaptureShas(ex) => vec![opt_ex(ex)],
            DownloadCrates(ex) => vec![opt_ex(ex)],
            FrobCargoTomls(ex) => vec![opt_ex(ex)],
            CaptureLockfiles(ex, tc) => vec![opt_ex(ex), opt_tc(tc)],

            PrepareExLocal(ex) => vec![opt_ex(ex)],
            DeleteAllTargetDirs(ex) => vec![opt_ex(ex)],
            DeleteAllResults(ex) => vec![opt_ex(ex)],
            FetchDeps(ex, tc) => vec![opt_ex(ex), opt_tc(tc)],
            PrepareAllToolchains(ex) => vec![opt_ex(ex)],

            Run(ex) => vec![opt_ex(ex)],
            RunTc(ex, tc) => vec![opt_ex(ex), req_tc(tc)],

            GenReport(ex) => vec![opt_ex(ex)],

            CreateDockerJob(cmd) => cmd_to_args(*cmd),
            StartJob(job) => vec![req_job(job)],
            WaitForJob(job) => vec![req_job(job)],
            RunJob(job) => vec![req_job(job)],
            RunJobAgain(job) => vec![req_job(job)],
            RunCmdForJob(job) => vec![req_job(job)],

            Say(msg) => vec![req_say_msg(msg)],
        }
    }

    pub fn args_to_cmd(args: &[String]) -> Result<Cmd> {
        let m = App::new("")
            .setting(AppSettings::NoBinaryName)
            .subcommands(clap_cmds())
            .get_matches_from(args);
        clap_args_to_cmd(&m)
    }

    use bmk::Arguable;

    impl Arguable for Cmd {
        fn from_args(args: Vec<String>) -> Result<Self> {
            args_to_cmd(&args)
        }

        fn to_args(self) -> Vec<String> {
            cmd_to_args(self)
        }
    }

    impl FromStr for ExMode {
        type Err = Error;

        fn from_str(s: &str) -> Result<ExMode> {
            Ok(match s {
                   "build-and-test" => ExMode::BuildAndTest,
                   "build-only" => ExMode::BuildOnly,
                   "check-only" => ExMode::CheckOnly,
                   "unstable-features" => ExMode::UnstableFeatures,
                   s => bail!("invalid ex-mode: {}", s),
               })
        }
    }

    impl ExMode {
        pub fn to_str(&self) -> &'static str {
            match *self {
                ExMode::BuildAndTest => "build-and-test",
                ExMode::BuildOnly => "build-only",
                ExMode::CheckOnly => "check-only",
                ExMode::UnstableFeatures => "unstable-features",
            }
        }
    }

    impl FromStr for ExCrateSelect {
        type Err = Error;

        fn from_str(s: &str) -> Result<ExCrateSelect> {
            Ok(match s {
                   "full" => ExCrateSelect::Full,
                   "demo" => ExCrateSelect::Demo,
                   "small-random" => ExCrateSelect::SmallRandom,
                   "top-100" => ExCrateSelect::Top100,
                   s => bail!("invalid crate-select: {}", s),
               })
        }
    }

    impl ExCrateSelect {
        pub fn to_str(&self) -> &'static str {
            match *self {
                ExCrateSelect::Full => "full",
                ExCrateSelect::Demo => "demo",
                ExCrateSelect::SmallRandom => "small-random",
                ExCrateSelect::Top100 => "top-100",
            }
        }
    }

    impl FromStr for Job {
        type Err = Error;

        fn from_str(job: &str) -> Result<Job> {
            Ok(Job(JobId(job.parse().chain_err(|| "parsing job id")?)))
        }
    }

    impl FromStr for Ex {
        type Err = Error;

        fn from_str(ex: &str) -> Result<Ex> {
            Ok(Ex(ex.to_string()))
        }
    }
}
