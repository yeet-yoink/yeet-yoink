use std::fmt::{Display, Formatter};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HealthState {
    Healthy,
    Degraded,
    Failed,
}

impl Display for HealthState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthState::Healthy => write!(f, "Healthy"),
            HealthState::Degraded => write!(f, "Degraded"),
            HealthState::Failed => write!(f, "Failed"),
        }
    }
}
