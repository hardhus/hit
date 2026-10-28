use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Pending,
    Success,
    Failure,
    /// Skipped, cancelled or otherwise inconclusive.
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

/// Rolled-up CI status of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSummary {
    pub state: CheckState,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
}

impl CheckSummary {
    /// Summarizes runs: any failure fails, otherwise any pending is pending.
    pub fn from_runs(runs: &[CheckRun]) -> Option<Self> {
        if runs.is_empty() {
            return None;
        }
        let count = |state| {
            u32::try_from(runs.iter().filter(|r| r.state == state).count()).unwrap_or(u32::MAX)
        };
        let (passed, failed, pending) = (
            count(CheckState::Success),
            count(CheckState::Failure),
            count(CheckState::Pending),
        );
        let state = if failed > 0 {
            CheckState::Failure
        } else if pending > 0 {
            CheckState::Pending
        } else if passed > 0 {
            CheckState::Success
        } else {
            CheckState::Neutral
        };
        Some(Self {
            state,
            passed,
            failed,
            pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(state: CheckState) -> CheckRun {
        CheckRun {
            name: "ci".into(),
            state,
            url: None,
        }
    }

    #[test]
    fn summary_prefers_failure_then_pending() {
        use CheckState::*;
        assert_eq!(CheckSummary::from_runs(&[]), None);
        let s = CheckSummary::from_runs(&[run(Success), run(Pending), run(Failure)]).unwrap();
        assert_eq!((s.state, s.passed, s.failed, s.pending), (Failure, 1, 1, 1));
        let s = CheckSummary::from_runs(&[run(Success), run(Pending)]).unwrap();
        assert_eq!(s.state, Pending);
        let s = CheckSummary::from_runs(&[run(Success), run(Neutral)]).unwrap();
        assert_eq!(s.state, Success);
    }
}
