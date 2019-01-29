mod db;
#[cfg(test)]
mod dummy;

use crate::config::Config;
use crate::crates::{Crate, GitHubRepo};
use crate::experiments::{Experiment, ExperimentChunk};
use crate::logs::LogStorage;
use crate::prelude::*;
pub use crate::results::db::{DatabaseDB, ProgressData};
#[cfg(test)]
pub use crate::results::dummy::DummyDB;
use crate::toolchain::Toolchain;
use std::collections::HashMap;
use std::{fmt, str::FromStr};

pub trait ReadResults {
    fn load_all_shas(&self, ex: &Experiment) -> Fallible<HashMap<GitHubRepo, String>>;
    fn load_log(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<Vec<u8>>>;
    fn load_test_result(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>>;
    fn load_all_shas_chunk(&self, ex: &ExperimentChunk) -> Fallible<HashMap<GitHubRepo, String>>;
    fn load_log_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<Vec<u8>>>;
    fn load_test_result_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>>;
}

pub trait WriteResults {
    fn get_result(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>>;
    fn record_sha(&self, ex: &Experiment, repo: &GitHubRepo, sha: &str) -> Fallible<()>;
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
        F: FnOnce() -> Fallible<TestResult>;
    fn get_result_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<Option<TestResult>>;
    fn record_sha_chunk(&self, ex: &ExperimentChunk, repo: &GitHubRepo, sha: &str) -> Fallible<()>;
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
        F: FnOnce() -> Fallible<TestResult>;
}

pub trait DeleteResults {
    fn delete_all_results(&self, ex: &Experiment) -> Fallible<()>;
    fn delete_result(&self, ex: &Experiment, toolchain: &Toolchain, krate: &Crate) -> Fallible<()>;
    fn delete_all_results_chunk(&self, ex: &ExperimentChunk) -> Fallible<()>;
    fn delete_result_chunk(
        &self,
        ex: &ExperimentChunk,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Fallible<()>;
}

macro_rules! test_result_enum {
    (pub enum $name:ident {
        with_reason { $($with_reason_name:ident(FailureReason) => $with_reason_repr:expr,)* }
        without_reason { $($reasonless_name:ident => $reasonless_repr:expr,)* }
    }) => {
        #[derive(Debug, PartialEq, Eq, Copy, Clone, Hash)]
        pub enum $name {
            $($with_reason_name(FailureReason),)*
            $($reasonless_name,)*
        }

        impl FromStr for $name {
            type Err = ::failure::Error;

            fn from_str(input: &str) -> Fallible<Self> {
                let parts: Vec<&str> = input.split(':').collect();

                if parts.len() == 1 {
                    match parts[0] {
                        $($with_reason_repr => Ok($name::$with_reason_name(FailureReason::Unknown)),)*
                        $($reasonless_repr => Ok($name::$reasonless_name),)*
                        other => Err(TestResultParseError::UnknownResult(other.into()).into()),
                    }
                } else if parts.len() == 2 {
                    match parts[0] {
                        $($reasonless_repr => Err(TestResultParseError::UnexpectedFailureReason.into()),)*
                        $($with_reason_repr => Ok($name::$with_reason_name(parts[1].parse()?)),)*
                        other => Err(TestResultParseError::UnknownResult(other.into()).into()),
                    }
                } else {
                    Err(TestResultParseError::TooManySegments.into())
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                match self {
                    $($name::$with_reason_name(reason) => write!(f, "{}:{}", $with_reason_repr, reason),)*
                    $($name::$reasonless_name => write!(f, "{}", $reasonless_repr),)*
                }
            }
        }
    }
}

#[derive(Debug, Fail)]
pub enum TestResultParseError {
    #[fail(display = "unknown test result: {}", _0)]
    UnknownResult(String),
    #[fail(display = "unexpected failure reason")]
    UnexpectedFailureReason,
    #[fail(display = "too many segments")]
    TooManySegments,
}

string_enum!(pub enum FailureReason {
    Unknown => "unknown",
    Broken => "broken",
    OOM => "oom",
    Timeout => "timeout",
});

impl FailureReason {
    pub(crate) fn is_spurious(self) -> bool {
        match self {
            FailureReason::Unknown | FailureReason::Broken => false,
            FailureReason::OOM | FailureReason::Timeout => true,
        }
    }
}

test_result_enum!(pub enum TestResult {
    with_reason {
        BuildFail(FailureReason) => "build-fail",
        TestFail(FailureReason) => "test-fail",
    }
    without_reason {
        TestSkipped => "test-skipped",
        TestPass => "test-pass",
        Error => "error",
    }
});

impl_serde_from_parse!(TestResult, expecting = "a test result");

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    #[test]
    fn test_test_result_parsing() {
        use super::{
            FailureReason::*,
            TestResult::{self, *},
        };

        macro_rules! test_from_str {
            ($($str:expr => $rust:expr,)*) => {
                $(
                    // Test parsing from string to rust
                    assert_eq!(TestResult::from_str($str).unwrap(), $rust);

                    // Test dumping from rust to string
                    assert_eq!(&$rust.to_string(), $str);

                    // Test dumping from rust to string to rust
                    assert_eq!(TestResult::from_str($rust.to_string().as_ref()).unwrap(), $rust);
                )*
            };
        }

        test_from_str! {
            "build-fail:unknown" => BuildFail(Unknown),
            "build-fail:oom" => BuildFail(OOM),
            "test-fail:timeout" => TestFail(Timeout),
            "test-pass" => TestPass,
            "error" => Error,
        }

        // Backward compatibility
        assert_eq!(
            TestResult::from_str("build-fail").unwrap(),
            BuildFail(Unknown)
        );

        assert!(TestResult::from_str("error:oom").is_err());
        assert!(TestResult::from_str("build-fail:pleasedonotaddthis").is_err());
    }
}
