use config::Config;
use db::{Database, QueryUtils};
use errors::*;
use experiments::{CapLints, CrateSelect, Experiment, Mode, Status};
use toolchain::Toolchain;

pub struct EditExperiment {
    pub name: String,
    pub toolchains: [Option<Toolchain>; 2],
    pub crates: Option<CrateSelect>,
    pub mode: Option<Mode>,
    pub cap_lints: Option<CapLints>,
    pub priority: Option<i32>,
}

impl EditExperiment {
    #[cfg(test)]
    pub fn dummy(name: &str) -> Self {
        EditExperiment {
            name: name.to_string(),
            toolchains: [None, None],
            mode: None,
            crates: None,
            cap_lints: None,
            priority: None,
        }
    }

    pub fn apply(mut self, db: &Database, config: &Config) -> Result<bool> {
        let mut ex = match Experiment::get(db, &self.name)? {
            Some(ex) => ex,
            None => return Err(ErrorKind::ExperimentNotFound(self.name).into()),
        };

        // Ensure no change is made to running or complete experiments
        if ex.status != Status::Queued {
            return Err(ErrorKind::CanEditOnlyQueuedExperiments.into());
        }

        db.transaction(|t| {
            let mut has_changed = false;

            // Try to update both toolchains
            for (i, col) in ["toolchain_start", "toolchain_end"].iter().enumerate() {
                if let Some(tc) = self.toolchains[i].take() {
                    ex.toolchains[i] = tc;

                    // Ensure no duplicate toolchain is inserted
                    if ex.toolchains[0] == ex.toolchains[1] {
                        return Err(ErrorKind::DuplicateToolchains.into());
                    }

                    let changes = t.execute(
                        &format!("UPDATE experiments SET {} = ?1 WHERE name = ?2;", col),
                        &[&ex.toolchains[i].to_string(), &self.name],
                    )?;
                    assert_eq!(changes, 1);

                    has_changed = true;
                }
            }

            // Try to update the list of crates
            if let Some(crates) = self.crates {
                let crates_vec = ::lists::get_crates(crates, config)?;

                // Recreate the list of crates without checking if it was the same
                // This is done to allow reloading the list of crates in an existing experiment
                t.execute(
                    "DELETE FROM experiment_crates WHERE experiment = ?1;",
                    &[&self.name],
                )?;
                for krate in &crates_vec {
                    t.execute(
                        "INSERT INTO experiment_crates (experiment, crate, skipped) \
                         VALUES (?1, ?2, ?3);",
                        &[
                            &self.name,
                            &::serde_json::to_string(&krate)?,
                            &config.should_skip(krate),
                        ],
                    )?;
                }

                ex.crates = crates_vec;
                has_changed = true;
            }

            // Try to update the mode
            if let Some(mode) = self.mode {
                let changes = t.execute(
                    "UPDATE experiments SET mode = ?1 WHERE name = ?2;",
                    &[&mode.to_str(), &self.name],
                )?;
                assert_eq!(changes, 1);

                ex.mode = mode;
                has_changed = true;
            }

            // Try to update the cap_lints
            if let Some(cap_lints) = self.cap_lints {
                let changes = t.execute(
                    "UPDATE experiments SET cap_lints = ?1 WHERE name = ?2;",
                    &[&cap_lints.to_str(), &self.name],
                )?;
                assert_eq!(changes, 1);

                ex.cap_lints = cap_lints;
                has_changed = true;
            }

            // Try to update the priority
            if let Some(priority) = self.priority {
                let changes = t.execute(
                    "UPDATE experiments SET priority = ?1 WHERE name = ?2;",
                    &[&priority, &self.name],
                )?;
                assert_eq!(changes, 1);

                ex.priority = priority;
                has_changed = true;
            }

            Ok(has_changed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::EditExperiment;
    use actions::CreateExperiment;
    use config::Config;
    use db::Database;
    use errors::*;
    use experiments::{CapLints, CrateSelect, Experiment, Mode, Status};
    use toolchain::{MAIN_TOOLCHAIN, TEST_TOOLCHAIN};

    #[test]
    fn test_edit_with_no_changes() {
        let db = Database::temp().unwrap();
        let config = Config::default();

        // Create a dummy experiment to edit
        CreateExperiment::dummy("foo").apply(&db, &config).unwrap();

        // Ensure no changes are applied when no changes are defined
        let has_changed = EditExperiment::dummy("foo").apply(&db, &config).unwrap();
        assert!(!has_changed);
    }

    #[test]
    fn test_edit_with_every_change() {
        let db = Database::temp().unwrap();
        let config = Config::default();

        // Create an experiment with the data we're going to change
        CreateExperiment {
            name: "foo".to_string(),
            toolchains: ["stable".parse().unwrap(), "beta".parse().unwrap()],
            mode: Mode::BuildAndTest,
            crates: CrateSelect::SmallRandom,
            cap_lints: CapLints::Forbid,
            priority: 0,
            github_issue: None,
        }.apply(&db, &config)
        .unwrap();

        // Change everything!
        EditExperiment {
            name: "foo".to_string(),
            toolchains: [
                Some("nightly-1970-01-01".parse().unwrap()),
                Some("nightly-1970-01-02".parse().unwrap()),
            ],
            mode: Some(Mode::CheckOnly),
            crates: Some(CrateSelect::Demo),
            cap_lints: Some(CapLints::Warn),
            priority: Some(10),
        }.apply(&db, &config)
        .unwrap();

        // And get the experiment to make sure data is changed
        let ex = Experiment::get(&db, "foo").unwrap().unwrap();

        assert_eq!(ex.toolchains[0], "nightly-1970-01-01".parse().unwrap());
        assert_eq!(ex.toolchains[1], "nightly-1970-01-02".parse().unwrap());
        assert_eq!(ex.mode, Mode::CheckOnly);
        assert_eq!(ex.cap_lints, CapLints::Warn);
        assert_eq!(ex.priority, 10);

        let demo = ::lists::get_crates(CrateSelect::Demo, &config).unwrap();
        assert_eq!(ex.crates, demo);
    }

    #[test]
    fn test_duplicate_toolchains() {
        let db = Database::temp().unwrap();
        let config = Config::default();

        // First create an experiment
        let mut dummy = CreateExperiment::dummy("foo");
        dummy.toolchains = [MAIN_TOOLCHAIN.clone(), TEST_TOOLCHAIN.clone()];
        dummy.apply(&db, &config).unwrap();

        // Then try to switch the second toolchain to MAIN_TOOLCHAIN
        let mut edit = EditExperiment::dummy("foo");
        edit.toolchains[1] = Some(MAIN_TOOLCHAIN.clone());

        let err = edit.apply(&db, &config).unwrap_err();
        match err.kind() {
            ErrorKind::DuplicateToolchains => {}
            other => panic!("received unexpected error: {}", other),
        }
    }

    #[test]
    fn test_editing_missing_experiment() {
        let db = Database::temp().unwrap();
        let config = Config::default();

        let err = EditExperiment::dummy("foo")
            .apply(&db, &config)
            .unwrap_err();
        match err.kind() {
            ErrorKind::ExperimentNotFound(name) => assert_eq!(name, "foo"),
            other => panic!("received unexpected error: {}", other),
        }
    }

    #[test]
    fn test_editing_running_experiment() {
        let db = Database::temp().unwrap();
        let config = Config::default();

        // Create an experiment and set it to running
        CreateExperiment::dummy("foo").apply(&db, &config).unwrap();
        let mut ex = Experiment::get(&db, "foo").unwrap().unwrap();
        ex.set_status(&db, Status::Running).unwrap();

        // Try to edit it
        let err = EditExperiment::dummy("foo")
            .apply(&db, &config)
            .unwrap_err();
        match err.kind() {
            ErrorKind::CanEditOnlyQueuedExperiments => {}
            other => panic!("received unexpected error: {}", other),
        }
    }
}
