use config::Config;
use crates::{Crate, GitHubRepo};
use errors::*;
use ex;
use file;
use handlebars::Handlebars;
use mime::{self, Mime};
use results::{ReadResults, TestResult};
use serde_json;
use std::{fs, io};
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::AsRef;
use std::fmt::{self, Display};
use std::fs::File;
use std::path::{Path, PathBuf};

mod s3;
pub use self::s3::{get_client_for_bucket, S3Prefix, S3Writer};

#[derive(Serialize, Deserialize)]
pub struct TestResults {
    crates: Vec<CrateResult>,
}

#[derive(Serialize, Deserialize)]
struct CrateResult {
    name: String,
    url: String,
    res: Comparison,
    runs: [Option<BuildTestResult>; 2],
}

#[derive(Serialize, Deserialize)]
enum Comparison {
    Regressed,
    Fixed,
    Skipped,
    Unknown,
    SameBuildFail,
    SameTestFail,
    SameTestSkipped,
    SameTestPass,
}

#[derive(Serialize, Deserialize)]
struct BuildTestResult {
    res: TestResult,
    log: String,
}

fn crate_to_path_fragment(krate: &Crate) -> PathBuf {
    match *krate {
        Crate::Registry(ref details) => PathBuf::new()
            .join("reg")
            .join(format!("{}-{}", details.name, details.version)),
        Crate::GitHub(ref repo) => PathBuf::new()
            .join("gh")
            .join(format!("{}.{}", repo.org, repo.name)),
    }
}

pub fn generate_report<DB: ReadResults>(
    db: &DB,
    config: &Config,
    ex: &ex::Experiment,
) -> Result<TestResults> {
    let shas = db.load_all_shas(ex)?;
    assert_eq!(ex.toolchains.len(), 2);

    let res = ex.crates
        .clone()
        .into_iter()
        .map(|krate| {
            // Any errors here will turn into unknown results
            let crate_results = ex.toolchains.iter().map(|tc| -> Result<BuildTestResult> {
                let res = db.load_test_result(ex, tc, &krate)?
                    .ok_or_else(|| "no result")?;

                Ok(BuildTestResult {
                    res,
                    log: crate_to_path_fragment(&krate).to_str().unwrap().to_string(),
                })
            });
            // Convert errors to Nones
            let mut crate_results = crate_results.map(|r| r.ok()).collect::<Vec<_>>();
            let crate2 = crate_results.pop().expect("");
            let crate1 = crate_results.pop().expect("");
            let comp = compare(config, &krate, &crate1, &crate2);

            Ok(CrateResult {
                name: crate_to_name(&krate, &shas)?,
                url: crate_to_url(&krate, &shas)?,
                res: comp,
                runs: [crate1, crate2],
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(TestResults { crates: res })
}

const PROGRESS_FRACTION: usize = 10; // write progress every ~1/N crates

fn write_logs<DB: ReadResults, W: ReportWriter>(
    db: &DB,
    ex: &ex::Experiment,
    dest: &W,
    config: &Config,
) -> Result<()> {
    let num_crates = ex.crates.len();
    let progress_every = (num_crates / PROGRESS_FRACTION) + 1;
    for (i, krate) in ex.crates.iter().enumerate() {
        if i % progress_every == 0 {
            info!("wrote logs for {}/{} crates", i, num_crates)
        }

        if config.should_skip(krate) {
            continue;
        }

        for tc in &ex.toolchains {
            let log_path = crate_to_path_fragment(krate).join("log.txt");
            let content = db.load_log(ex, tc, krate)
                .and_then(|c| c.ok_or_else(|| "missing logs".into()))
                .chain_err(|| format!("failed to read log of {} on {}", krate, tc.to_string()))?;
            dest.write_string(log_path, content.into(), &mime::TEXT_PLAIN_UTF_8)?;
        }
    }
    Ok(())
}

pub fn gen<DB: ReadResults, W: ReportWriter + Display>(
    db: &DB,
    ex: &ex::Experiment,
    dest: &W,
    config: &Config,
) -> Result<()> {
    let res = generate_report(db, config, ex)?;

    info!("writing results to {}", dest);
    info!("writing metadata");
    dest.write_string(
        "results.json",
        serde_json::to_string(&res)?.into(),
        &mime::APPLICATION_JSON,
    )?;
    dest.write_string(
        "config.json",
        serde_json::to_string(&ex)?.into(),
        &mime::APPLICATION_JSON,
    )?;

    info!("writing html files");
    write_html_files(dest)?;
    info!("writing logs");
    write_logs(db, ex, dest, config)?;

    Ok(())
}

fn crate_to_name(c: &Crate, shas: &HashMap<GitHubRepo, String>) -> Result<String> {
    Ok(match *c {
        Crate::Registry(ref details) => format!("{}-{}", details.name, details.version),
        Crate::GitHub(ref repo) => {
            let sha = shas.get(repo)
                .ok_or_else(|| format!("missing sha for GitHub repo {}", repo.slug()))?
                .as_str();
            format!("{}.{}.{}", repo.org, repo.name, sha)
        }
    })
}

fn crate_to_url(c: &Crate, shas: &HashMap<GitHubRepo, String>) -> Result<String> {
    Ok(match *c {
        Crate::Registry(ref details) => format!(
            "https://crates.io/crates/{}/{}",
            details.name, details.version
        ),
        Crate::GitHub(ref repo) => {
            let sha = shas.get(repo)
                .ok_or_else(|| format!("missing sha for GitHub repo {}", repo.slug()))?
                .as_str();
            format!("https://github.com/{}/{}/tree/{}", repo.org, repo.name, sha)
        }
    })
}

fn compare(
    config: &Config,
    krate: &Crate,
    r1: &Option<BuildTestResult>,
    r2: &Option<BuildTestResult>,
) -> Comparison {
    use results::TestResult::*;
    match (r1, r2) {
        (
            &Some(BuildTestResult { res: ref res1, .. }),
            &Some(BuildTestResult { res: ref res2, .. }),
        ) => match (res1, res2) {
            (&BuildFail, &BuildFail) => Comparison::SameBuildFail,
            (&TestFail, &TestFail) => Comparison::SameTestFail,
            (&TestSkipped, &TestSkipped) => Comparison::SameTestSkipped,
            (&TestPass, &TestPass) => Comparison::SameTestPass,
            (&BuildFail, &TestFail)
            | (&BuildFail, &TestSkipped)
            | (&BuildFail, &TestPass)
            | (&TestFail, &TestPass) => Comparison::Fixed,
            (&TestPass, &TestFail)
            | (&TestPass, &BuildFail)
            | (&TestSkipped, &BuildFail)
            | (&TestFail, &BuildFail) => Comparison::Regressed,
            (&TestFail, &TestSkipped)
            | (&TestPass, &TestSkipped)
            | (&TestSkipped, &TestFail)
            | (&TestSkipped, &TestPass) => {
                panic!("can't compare {} and {}", res1, res2);
            }
        },
        _ if config.should_skip(krate) => Comparison::Skipped,
        _ => Comparison::Unknown,
    }
}

#[derive(Serialize, Deserialize)]
pub struct Context {
    pub config_url: String,
    pub results_url: String,
    pub static_url: String,
}

fn write_html_files<W: ReportWriter>(dest: &W) -> Result<()> {
    let html_in = include_str!("../../template/report.html");
    let js_in = include_str!("../../static/report.js");
    let css_in = include_str!("../../static/report.css");
    let html_out = "index.html";
    let js_out = "report.js";
    let css_out = "report.css";

    let context = Context {
        config_url: "config.json".into(),
        results_url: "results.json".into(),
        static_url: "".into(),
    };
    let html = Handlebars::new()
        .template_render(html_in, &context)
        .chain_err(|| "Couldn't render template")?;

    dest.write_string(&html_out, html.into(), &mime::TEXT_HTML)?;
    dest.write_string(&js_out, js_in.into(), &mime::TEXT_JAVASCRIPT)?;
    dest.write_string(&css_out, css_in.into(), &mime::TEXT_CSS)?;

    Ok(())
}

pub trait ReportWriter {
    fn write_string<P: AsRef<Path>>(&self, path: P, s: Cow<str>, mime: &Mime) -> Result<()>;
    fn copy<P: AsRef<Path>, R: io::Read>(&self, r: &mut R, path: P, mime: &Mime) -> Result<()>;
}

pub struct FileWriter(PathBuf);

impl FileWriter {
    pub fn create(dest: PathBuf) -> Result<FileWriter> {
        fs::create_dir_all(&dest)?;
        Ok(FileWriter(dest))
    }
    fn create_prefix(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(self.0.join(parent))?;
        }
        Ok(())
    }
}

impl ReportWriter for FileWriter {
    fn write_string<P: AsRef<Path>>(&self, path: P, s: Cow<str>, _: &Mime) -> Result<()> {
        self.create_prefix(path.as_ref())?;
        file::write_string(&self.0.join(path.as_ref()), s.as_ref())
    }
    fn copy<P: AsRef<Path>, R: io::Read>(&self, r: &mut R, path: P, _: &Mime) -> Result<()> {
        self.create_prefix(path.as_ref())?;
        io::copy(r, &mut File::create(self.0.join(path.as_ref()))?)?;
        Ok(())
    }
}

impl Display for FileWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.display().fmt(f)
    }
}
