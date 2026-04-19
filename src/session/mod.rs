pub mod stats;

use crate::error::{AppError, Result};

/// Lifecycle states of a walking session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No session active; belt is idle.
    Idle,
    /// Belt is running; session is accumulating stats.
    Running,
    /// Session exists but belt is stopped (manually or auto).
    Paused,
}

/// Events that drive state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    Start,
    Pause,
    Resume,
    Stop,
}

impl SessionState {
    /// Apply an event and return the next state.
    /// Returns `Err` for invalid transitions.
    pub fn transition(self, event: SessionEvent) -> Result<SessionState> {
        use SessionState::*;
        use SessionEvent::*;

        match (self, event) {
            (Idle, Start)        => Ok(Running),
            (Running, Pause)     => Ok(Paused),
            (Running, Stop)      => Ok(Idle),
            (Paused, Resume)     => Ok(Running),
            (Paused, Stop)       => Ok(Idle),
            (state, ev) => Err(AppError::InvalidTransition(format!(
                "{state:?} + {ev:?}"
            ))),
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::*;
    use SessionEvent::*;

    #[test]
    fn test_idle_start_becomes_running() {
        assert_eq!(Idle.transition(Start).unwrap(), Running);
    }

    #[test]
    fn test_running_pause_becomes_paused() {
        assert_eq!(Running.transition(Pause).unwrap(), Paused);
    }

    #[test]
    fn test_paused_resume_becomes_running() {
        assert_eq!(Paused.transition(Resume).unwrap(), Running);
    }

    #[test]
    fn test_running_stop_becomes_idle() {
        assert_eq!(Running.transition(Stop).unwrap(), Idle);
    }

    #[test]
    fn test_paused_stop_becomes_idle() {
        assert_eq!(Paused.transition(Stop).unwrap(), Idle);
    }

    #[test]
    fn test_idle_pause_is_invalid() {
        assert!(Idle.transition(Pause).is_err());
    }

    #[test]
    fn test_idle_resume_is_invalid() {
        assert!(Idle.transition(Resume).is_err());
    }

    #[test]
    fn test_running_start_is_invalid() {
        assert!(Running.transition(Start).is_err());
    }

    #[test]
    fn test_paused_start_is_invalid() {
        assert!(Paused.transition(Start).is_err());
    }

    #[test]
    fn test_paused_pause_is_invalid() {
        assert!(Paused.transition(Pause).is_err());
    }
}
