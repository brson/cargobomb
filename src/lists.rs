use crates::{Crate, RegistryCrate};
use dirs::LIST_DIR;
use errors::*;
use file;
use gh;
use registry;
use semver::{Version, VersionReq};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use util;

pub fn create_all_lists(full: bool) -> Result<()> {
    RecentList::create()?;
    HotList::create()?;
    PopList::create()?;
    if full {
        GitHubCandidateList::create()?;
        GitHubAppList::create()?;
    } else {
        create_gh_candidate_list_from_cache()?;
        create_gh_app_list_from_cache()?;
    }

    Ok(())
}

pub trait List {
    fn create() -> Result<()>;
    fn read() -> Result<Vec<Crate>>;
    fn path() -> PathBuf;
}

struct RecentList;

impl List for RecentList {
    fn create() -> Result<()> {
        info!("creating recent list");
        fs::create_dir_all(&*LIST_DIR)?;

        let crates = registry::crates_index_registry()?.crates().map(|crate_| {
            (
                crate_.name().to_owned(),
                crate_.latest_version().version().to_owned(),
            )
        });
        write_crate_list(&Self::path(), crates)?;
        info!("recent crates written to {}", Self::path().display());
        Ok(())
    }

    fn read() -> Result<Vec<Crate>> {
        let lines = file::read_lines(&Self::path())
            .chain_err(|| "unable to read recent list. run `crater create-lists`?")?;
        split_crate_lines(&lines)
    }

    fn path() -> PathBuf {
        LIST_DIR.join("recent-crates.txt")
    }
}

// (String, String) corresponds to (crate name, crate version)
fn write_crate_list<I>(path: &Path, crates: I) -> Result<()>
where
    I: Iterator<Item = (String, String)>,
{
    let strings = crates
        .map(|(name, version)| format!("{}:{}", name, version))
        .collect::<Vec<_>>();
    file::write_lines(path, &strings)
}

fn split_crate_lines(lines: &[String]) -> Result<Vec<Crate>> {
    Ok(lines
        .iter()
        .filter_map(|line| {
            line.find(':')
                .map(|i| (line[..i].to_string(), line[i + 1..].to_string()))
        })
        .map(|(name, version)| Crate::Registry(RegistryCrate { name, version }))
        .collect())
}

pub struct PopList;

impl List for PopList {
    fn create() -> Result<()> {
        info!("creating hot list");
        fs::create_dir_all(&*LIST_DIR)?;

        let index = registry::crates_index_registry()?;
        info!("mapping reverse deps");

        // Count the reverse deps of each crate
        let mut counts = HashMap::new();
        for crate_ in index.crates() {
            // Find all the crates this crate depends on
            let mut seen = HashSet::new();
            for version in crate_.versions() {
                seen.extend(version.dependencies().iter().map(|d| d.name().to_string()))
            }
            // Each of those crates gets +1
            for c in seen.drain() {
                let count = counts.entry(c).or_insert(0);
                *count += 1;
            }
        }

        let mut crates = index.crates().collect::<Vec<_>>();

        crates.sort_by(|a, b| {
            let count_a = counts.get(a.name()).cloned().unwrap_or(0);
            let count_b = counts.get(b.name()).cloned().unwrap_or(0);
            count_b.cmp(&count_a)
        });
        let crates = crates.into_iter().map(|crate_| {
            (
                crate_.name().to_owned(),
                crate_.latest_version().version().to_owned(),
            )
        });
        write_crate_list(&Self::path(), crates)?;
        info!("pop crates written to {}", Self::path().display());
        Ok(())
    }

    fn read() -> Result<Vec<Crate>> {
        let lines = file::read_lines(&Self::path())
            .chain_err(|| "unable to read pop list. run `crater create-lists`?")?;
        split_crate_lines(&lines)
    }

    fn path() -> PathBuf {
        LIST_DIR.join("pop-crates.txt")
    }
}

struct HotList;

impl List for HotList {
    fn create() -> Result<()> {
        info!("creating hot list");
        fs::create_dir_all(&*LIST_DIR)?;

        let index = registry::crates_index_registry()?;

        // We're going to map reverse dependency counts of all crate versions.

        // Create a map from name to versions, recent to oldest
        let mut crate_map = HashMap::new();
        for crate_ in index.crates() {
            let versions: Vec<_> = crate_
                .versions()
                .iter()
                .rev()
                .take(10)
                .map(|v| (v.version().to_string(), 0))
                .collect();
            crate_map.insert(crate_.name().to_string(), versions);
        }

        info!("mapping reverse deps");
        // For each crate's dependency mark which revisions of the dep satisfy
        // semver
        for crate_ in index.crates() {
            for version in crate_.versions() {
                for dependency in version.dependencies().iter() {
                    if let Some(ref mut dep_versions) = crate_map.get_mut(dependency.name()) {
                        let semver_req = VersionReq::parse(dependency.requirement());
                        for &mut (ref rev, ref mut count) in dep_versions.iter_mut() {
                            let semver_rev = Version::parse(rev);
                            if let (&Ok(ref req), Ok(ref rev)) = (&semver_req, semver_rev) {
                                if req.matches(rev) {
                                    *count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        info!("calculating most popular crate versions");
        // Take the version of each crate that satisfies the most rev deps
        let mut hot_crates = Vec::new();
        for crate_ in index.crates() {
            if let Some(dep_versions) = crate_map.get(crate_.latest_version().name()) {
                let mut best_version = String::new();
                let mut max_rev_deps = 0;
                for version in dep_versions {
                    // Only pick versions that have more than 0 rev deps,
                    // and prefer newer revisions (earlier in the list).
                    if version.1 > max_rev_deps {
                        best_version = version.0.to_string();
                        max_rev_deps = version.1;
                    }
                }
                if !best_version.is_empty() {
                    hot_crates.push((crate_.latest_version().name().to_string(), best_version));
                }
            }
        }

        write_crate_list(&Self::path(), hot_crates.into_iter())?;
        info!("hot crates written to {}", Self::path().display());
        Ok(())
    }

    fn read() -> Result<Vec<Crate>> {
        let lines = file::read_lines(&Self::path())
            .chain_err(|| "unable to read hot list. run `crater create-lists`?")?;
        split_crate_lines(&lines)
    }

    fn path() -> PathBuf {
        LIST_DIR.join("hot-crates.txt")
    }
}

struct GitHubCandidateList;

impl List for GitHubCandidateList {
    fn create() -> Result<()> {
        info!("creating gh candidate list");
        fs::create_dir_all(&*LIST_DIR)?;

        let candidates = gh::get_candidate_repos()?;
        file::write_lines(&Self::path(), &candidates)?;
        info!("candidate repos written to {}", Self::path().display());
        Ok(())
    }

    fn read() -> Result<Vec<Crate>> {
        Ok(file::read_lines(&Self::path())
            .chain_err(|| "unable to read gh-candidates list. run `crater create-lists`?")?
            .into_iter()
            .map(|line| line.parse().map(Crate::GitHub))
            .collect::<Result<Vec<Crate>>>()?)
    }

    fn path() -> PathBuf {
        LIST_DIR.join("gh-candidates.txt")
    }
}

fn gh_candidate_cache_path() -> PathBuf {
    Path::new("gh-candidates.txt").into()
}

fn create_gh_candidate_list_from_cache() -> Result<()> {
    info!("creating gh candidate list from cache");
    fs::create_dir_all(&*LIST_DIR)?;
    info!(
        "copying {} to {}",
        gh_candidate_cache_path().display(),
        GitHubCandidateList::path().display()
    );
    fs::copy(&gh_candidate_cache_path(), &GitHubCandidateList::path())?;
    Ok(())
}

struct GitHubAppList;

impl List for GitHubAppList {
    fn create() -> Result<()> {
        let crates = GitHubCandidateList::read()?;
        let delay = 100;

        info!(
            "testing {} repos. {}ms+",
            crates.len(),
            crates.len() * delay
        );

        // Look for Cargo.lock files in the Rust repos we're aware of
        let mut apps = Vec::new();
        for krate in crates {
            let repo = match krate {
                Crate::GitHub(ref repo) => repo,
                Crate::Registry(_) => unreachable!(),
            };
            if gh::is_rust_app(&repo.slug())? {
                apps.push(repo.url());
            }
            thread::sleep(Duration::from_millis(delay as u64));
        }

        file::write_lines(&Self::path(), &apps)?;
        info!("rust apps written to {}", Self::path().display());
        Ok(())
    }

    fn read() -> Result<Vec<Crate>> {
        Ok(file::read_lines(&GitHubAppList::path())
            .chain_err(|| "unable to read gh-app list. run `crater create-lists`?")?
            .into_iter()
            .map(|line| line.parse().map(Crate::GitHub))
            .collect::<Result<Vec<Crate>>>()?)
    }

    fn path() -> PathBuf {
        LIST_DIR.join("gh-apps.txt")
    }
}

fn gh_app_cache_path() -> PathBuf {
    Path::new("gh-apps.txt").into()
}

fn create_gh_app_list_from_cache() -> Result<()> {
    info!("creating gh app list from cache");
    fs::create_dir_all(&*LIST_DIR)?;
    info!(
        "copying {} to {}",
        gh_app_cache_path().display(),
        GitHubAppList::path().display()
    );
    fs::copy(&gh_app_cache_path(), &GitHubAppList::path())?;
    Ok(())
}

pub fn read_all_lists() -> Result<Vec<Crate>> {
    let mut all = HashSet::new();

    match RecentList::read() {
        Ok(recent) => all.extend(recent.into_iter()),
        Err(e) => util::report_error(&e),
    }
    match HotList::read() {
        Ok(hot) => all.extend(hot.into_iter()),
        Err(e) => util::report_error(&e),
    }
    match GitHubAppList::read() {
        Ok(gh_apps) => all.extend(gh_apps.into_iter()),
        Err(e) => util::report_error(&e),
    }

    if all.is_empty() {
        bail!("no crates loaded. run `crater prepare-local`?");
    }

    let mut all: Vec<_> = all.drain().collect();
    all.sort();
    Ok(all)
}
