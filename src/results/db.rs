use crate::config::Config;
use crate::crates::{Crate, GitHubRepo};
use crate::db::{Database, QueryUtils};
use crate::experiments::{Experiment, ExperimentChunk};
use crate::logs::{self, LogStorage};
use crate::prelude::*;
use crate::results::{DeleteResults, ReadResults, TestResult, WriteResults};
use crate::toolchain::Toolchain;
use base64;
use log::LevelFilter;
use serde_json;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct TaskResult {
    #[serde(rename = "crate")]
    pub krate: Crate,
    pub toolchain: Toolchain,
    pub result: TestResult,
    pub log: String,
}

#[derive(Deserialize)]
pub struct ProgressData {
    pub results: Vec<TaskResult>,
    pub shas: Vec<(GitHubRepo, String)>,
}

impl ProgressData {
    pub fn merge(&mut self, mut other: ProgressData) {
        self.results.append(&mut other.results);
        self.shas.append(&mut other.shas);
    }
}

pub struct DatabaseDB<'a> {
    db: &'a Database,
}

impl<'a> DatabaseDB<'a> {
    pub fn new(db: &'a Database) -> Self {
        DatabaseDB { db }
    }

    pub fn store(&self, ex: &Experiment, data: &ProgressData) -> Fallible<()> {
        for result in &data.results {
            self.store_result(
                ex,
                &result.krate,
                &result.toolchain,
                result.result,
                &base64::decode(&result.log).with_context(|_| "invalid base64 log provided")?,
            )?;
        }

        for &(ref repo, ref sha) in &data.shas {
            self.record_sha(ex, repo, sha)?;
        }

        Ok(())
    }

    fn store_result(
        &self,
        ex: &Experiment,
        krate: &Crate,
        toolchain: &Toolchain,
        res: TestResult,
        log: &[u8],
    ) -> Fallible<()> {
        self.db.execute(
            "INSERT INTO results (experiment, crate, toolchain, result, log) \
             VALUES (?1, ?2, ?3, ?4, ?5);",
            &[
                &ex.name,
                &serde_json::to_string(krate)?,
                &toolchain.to_string(),
                &res.to_string(),
                &log,
            ],
        )?;
        Ok(())
    }

    fn store_result_chunk(
        &self,
        ex: &ExperimentChunk,
        krate: &Crate,
        toolchain: &Toolchain,
        res: TestResult,
        log: &[u8],
    ) -> Fallible<()> {
        self.db.execute(
            "INSERT INTO results (experiment, crate, toolchain, result, log) \
             VALUES (?1, ?2, ?3, ?4, ?5);",
            &[
                &ex.name,
                &serde_json::to_string(krate)?,
                &toolchain.to_string(),
                &res.to_string(),
                &log,
            ],
        )?;
        Ok(())
    }

    pub fn load_all_results(&self, ex: &Experiment) -> Fallible<ProgressData> {
        let shas = self
            .load_all_shas(ex)?
            .iter()
            .map(|(repo, sha)| (repo.clone(), sha.clone()))
            .collect();
        let results: Vec<TaskResult> = ex
            .crates
            .iter()
            .cloned()
            .map(move |krate| (krate, ex.toolchains[0].clone()))
            .chain(
                ex.crates
                    .iter()
                    .cloned()
                    .map(move |krate| (krate, ex.toolchains[1].clone())),
            )
            .map(move |(krate, toolchain)| {
                Ok(TaskResult {
                    krate: krate.clone(),
                    toolchain: toolchain.clone(),
                    result: self
                        .load_test_result(ex, &toolchain, &krate)?
                        .ok_or_else(|| err_msg("could not load crate result"))?,
                    log: String::from_utf8(
                        self.load_log(ex, &toolchain, &krate)?
                            .ok_or_else(|| err_msg("could not load log"))?,
                    )?,
                })
            })
            .filter_map(|task_result: Fallible<TaskResult>| task_result.ok())
            .collect();
        Ok(ProgressData { results, shas })
    }
}

impl<'a> ReadResults for DatabaseDB<'a> {
    fn load_all_shas(&self, ex: &Experiment) -> Fallible<HashMap<GitHubRepo, String>> {
        Ok(self
            .db
            .query(
                "SELECT * FROM shas WHERE experiment = ?1;",
                &[&ex.name],
                |row| {
                    (
                        GitHubRepo {
                            org: row.get("org"),
                            name: row.get("name"),
                        },
                        row.get("sha"),
                    )
                },
            )?
            .into_iter()
            .collect())
    }

    fn load_log(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<Vec<u8>>> {
        Ok(self.db.get_row(
            "SELECT log FROM results \
             WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3 \
             LIMIT 1;",
            &[
                &ex.name,
                &toolchain.to_string(),
                &serde_json::to_string(krate)?,
            ],
            |row| row.get("log"),
        )?)
    }

    fn load_test_result(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>> {
        let result: Option<String> = self
            .db
            .query(
                "SELECT result FROM results \
                 WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3 \
                 LIMIT 1;",
                &[
                    &ex.name,
                    &toolchain.to_string(),
                    &serde_json::to_string(krate)?,
                ],
                |row| row.get("result"),
            )?
            .pop();

        if let Some(res) = result {
            Ok(Some(res.parse()?))
        } else {
            Ok(None)
        }
    }

    fn load_all_shas_chunk(&self, ex: &ExperimentChunk) -> Fallible<HashMap<GitHubRepo, String>> {
        Ok(self
            .db
            .query(
                "SELECT * FROM shas WHERE experiment = ?1;",
                &[&ex.name],
                |row| {
                    (
                        GitHubRepo {
                            org: row.get("org"),
                            name: row.get("name"),
                        },
                        row.get("sha"),
                    )
                },
            )?
            .into_iter()
            .collect())
    }

    fn load_log_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<Vec<u8>>> {
        Ok(self.db.get_row(
            "SELECT log FROM results \
             WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3 \
             LIMIT 1;",
            &[
                &ex.name,
                &toolchain.to_string(),
                &serde_json::to_string(krate)?,
            ],
            |row| row.get("log"),
        )?)
    }

    fn load_test_result_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>> {
        let result: Option<String> = self
            .db
            .query(
                "SELECT result FROM results \
                 WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3 \
                 LIMIT 1;",
                &[
                    &ex.name,
                    &toolchain.to_string(),
                    &serde_json::to_string(krate)?,
                ],
                |row| row.get("result"),
            )?
            .pop();

        if let Some(res) = result {
            Ok(Some(res.parse()?))
        } else {
            Ok(None)
        }
    }
}

impl<'a> WriteResults for DatabaseDB<'a> {
    fn get_result(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>> {
        self.load_test_result(ex, toolchain, krate)
    }

    fn record_sha(&self, ex: &Experiment, repo: &GitHubRepo, sha: &str) -> Fallible<()> {
        self.db.execute(
            "INSERT INTO shas (experiment, org, name, sha) VALUES (?1, ?2, ?3, ?4)",
            &[&ex.name, &repo.org, &repo.name, &sha],
        )?;

        Ok(())
    }

    fn record_result<F>(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
        existing_logs: Option<LogStorage>,
        config: &Config,
        f: F,
    ) -> Fallible<TestResult>
    where
        F: FnOnce() -> Fallible<TestResult>,
    {
        let storage = existing_logs.unwrap_or_else(|| LogStorage::new(LevelFilter::Info, config));
        let result = logs::capture(&storage, f)?;
        let output = storage.to_string();
        self.store_result(ex, krate, toolchain, result, output.as_bytes())?;
        Ok(result)
    }

    fn get_result_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>> {
        self.load_test_result_chunk(ex, toolchain, krate)
    }

    fn record_sha_chunk(&self, ex: &ExperimentChunk, repo: &GitHubRepo, sha: &str) -> Fallible<()> {
        self.db.execute(
            "INSERT INTO shas (experiment, org, name, sha) VALUES (?1, ?2, ?3, ?4)",
            &[&ex.name, &repo.org, &repo.name, &sha],
        )?;

        Ok(())
    }

    fn record_result_chunk<F>(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
        existing_logs: Option<LogStorage>,
        config: &Config,
        f: F,
    ) -> Fallible<TestResult>
    where
        F: FnOnce() -> Fallible<TestResult>,
    {
        let storage = existing_logs.unwrap_or_else(|| LogStorage::new(LevelFilter::Info, config));
        let result = logs::capture(&storage, f)?;
        let output = storage.to_string();
        self.store_result_chunk(ex, krate, toolchain, result, output.as_bytes())?;
        Ok(result)
    }
}

impl<'a> DeleteResults for DatabaseDB<'a> {
    fn delete_all_results(&self, ex: &Experiment) -> Fallible<()> {
        self.db
            .execute("DELETE FROM results WHERE experiment = ?1;", &[&ex.name])?;
        Ok(())
    }

    fn delete_result(&self, ex: &Experiment, tc: &Toolchain, krate: &Crate) -> Fallible<()> {
        self.db.execute(
            "DELETE FROM results WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3;",
            &[
                &ex.name,
                &tc.to_string(),
                &serde_json::to_string(krate).unwrap(),
            ],
        )?;
        Ok(())
    }

    fn delete_all_results_chunk(&self, ex: &ExperimentChunk) -> Fallible<()> {
        self.db
            .execute("DELETE FROM results WHERE experiment = ?1;", &[&ex.name])?;
        Ok(())
    }

    fn delete_result_chunk(
        &self,
        ex: &ExperimentChunk,
        tc: &Toolchain,
        krate: &Crate,
    ) -> Fallible<()> {
        self.db.execute(
            "DELETE FROM results WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3;",
            &[
                &ex.name,
                &tc.to_string(),
                &serde_json::to_string(krate).unwrap(),
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseDB, ProgressData, TaskResult};
    use crate::actions::{Action, ActionsCtx, CreateExperiment};
    use crate::config::Config;
    use crate::crates::{Crate, GitHubRepo, RegistryCrate};
    use crate::db::Database;
    use crate::experiments::Experiment;
    use crate::prelude::*;
    use crate::results::{DeleteResults, FailureReason, ReadResults, TestResult, WriteResults};
    use crate::toolchain::{MAIN_TOOLCHAIN, TEST_TOOLCHAIN};
    use base64;

    #[test]
    fn test_shas() {
        let db = Database::temp().unwrap();
        let results = DatabaseDB::new(&db);
        let config = Config::default();
        let ctx = ActionsCtx::new(&db, &config);

        crate::crates::lists::setup_test_lists(&db, &config).unwrap();

        // Create a dummy experiment to attach the results to
        CreateExperiment::dummy("dummy").apply(&ctx).unwrap();
        let ex = Experiment::get(&db, "dummy").unwrap().unwrap();

        // Define some dummy GitHub repositories
        let repo1 = GitHubRepo {
            org: "foo".to_string(),
            name: "bar".to_string(),
        };
        let repo2 = GitHubRepo {
            org: "foo".to_string(),
            name: "baz".to_string(),
        };

        // Store some SHAs for those repos
        results
            .record_sha(&ex, &repo1, "0000000000000000000000000000000000000000")
            .unwrap();
        results
            .record_sha(&ex, &repo2, "ffffffffffffffffffffffffffffffffffffffff")
            .unwrap();

        // Ensure all the SHAs were recorded correctly
        let shas = results.load_all_shas(&ex).unwrap();
        assert_eq!(shas.len(), 2);
        assert_eq!(
            shas.get(&repo1).unwrap(),
            "0000000000000000000000000000000000000000"
        );
        assert_eq!(
            shas.get(&repo2).unwrap(),
            "ffffffffffffffffffffffffffffffffffffffff"
        );

        // Ensure results are cleanly overridden when recording the same repo again
        results
            .record_sha(&ex, &repo1, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap();

        let shas = results.load_all_shas(&ex).unwrap();
        assert_eq!(shas.len(), 2);
        assert_eq!(
            shas.get(&repo1).unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            shas.get(&repo2).unwrap(),
            "ffffffffffffffffffffffffffffffffffffffff"
        );
    }

    #[test]
    fn test_results() {
        crate::logs::init_test();

        let db = Database::temp().unwrap();
        let results = DatabaseDB::new(&db);
        let config = Config::default();
        let ctx = ActionsCtx::new(&db, &config);

        crate::crates::lists::setup_test_lists(&db, &config).unwrap();

        // Create a dummy experiment to attach the results to
        CreateExperiment::dummy("dummy").apply(&ctx).unwrap();
        let ex = Experiment::get(&db, "dummy").unwrap().unwrap();

        let krate = Crate::Registry(RegistryCrate {
            name: "lazy_static".into(),
            version: "1".into(),
        });

        // Record a result with a message in it
        results
            .record_result(&ex, &MAIN_TOOLCHAIN, &krate, None, &config, || {
                info!("hello world");
                Ok(TestResult::TestPass)
            })
            .unwrap();

        // Ensure the data is recorded correctly
        assert_eq!(
            results
                .load_test_result(&ex, &MAIN_TOOLCHAIN, &krate)
                .unwrap(),
            Some(TestResult::TestPass)
        );
        assert!(String::from_utf8_lossy(
            &results
                .load_log(&ex, &MAIN_TOOLCHAIN, &krate)
                .unwrap()
                .unwrap()
        )
        .contains("hello world"));

        // Ensure no data is returned for missing results
        assert!(results
            .load_test_result(&ex, &TEST_TOOLCHAIN, &krate)
            .unwrap()
            .is_none());
        assert!(results
            .get_result(&ex, &TEST_TOOLCHAIN, &krate)
            .unwrap()
            .is_none());
        assert!(results
            .load_log(&ex, &TEST_TOOLCHAIN, &krate)
            .unwrap()
            .is_none());

        // Add another result
        results
            .record_result(&ex, &TEST_TOOLCHAIN, &krate, None, &config, || {
                info!("Another log message!");
                Ok(TestResult::TestFail(FailureReason::Unknown))
            })
            .unwrap();
        assert_eq!(
            results.get_result(&ex, &TEST_TOOLCHAIN, &krate).unwrap(),
            Some(TestResult::TestFail(FailureReason::Unknown))
        );

        // Test deleting the newly-added result
        results.delete_result(&ex, &TEST_TOOLCHAIN, &krate).unwrap();
        assert!(results
            .get_result(&ex, &TEST_TOOLCHAIN, &krate)
            .unwrap()
            .is_none());
        assert_eq!(
            results.get_result(&ex, &MAIN_TOOLCHAIN, &krate).unwrap(),
            Some(TestResult::TestPass)
        );

        // Test deleting all the remaining results
        results.delete_all_results(&ex).unwrap();
        assert!(results
            .get_result(&ex, &MAIN_TOOLCHAIN, &krate)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_store() {
        let db = Database::temp().unwrap();
        let results = DatabaseDB::new(&db);
        let config = Config::default();
        let ctx = ActionsCtx::new(&db, &config);

        crate::crates::lists::setup_test_lists(&db, &config).unwrap();

        // Create a dummy experiment to attach the results to
        CreateExperiment::dummy("dummy").apply(&ctx).unwrap();
        let ex = Experiment::get(&db, "dummy").unwrap().unwrap();

        let krate = Crate::Registry(RegistryCrate {
            name: "lazy_static".into(),
            version: "1".into(),
        });

        // Store a result and some SHAs
        results
            .store(
                &ex,
                &ProgressData {
                    results: vec![TaskResult {
                        krate: krate.clone(),
                        toolchain: MAIN_TOOLCHAIN.clone(),
                        result: TestResult::TestPass,
                        log: base64::encode("foo"),
                    }],
                    shas: vec![
                        (
                            GitHubRepo {
                                org: "foo".into(),
                                name: "bar".into(),
                            },
                            "42".into(),
                        ),
                        (
                            GitHubRepo {
                                org: "foo".into(),
                                name: "baz".into(),
                            },
                            "beef".into(),
                        ),
                    ],
                },
            )
            .unwrap();

        assert_eq!(
            results.load_log(&ex, &MAIN_TOOLCHAIN, &krate).unwrap(),
            Some("foo".as_bytes().to_vec())
        );
        assert_eq!(
            results
                .load_test_result(&ex, &MAIN_TOOLCHAIN, &krate)
                .unwrap(),
            Some(TestResult::TestPass)
        );
    }
}
