//! Membership state machine: states, transitions, and suspension tracking.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::keys::PublicKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MembershipState {
    Invited,
    Active,
    Suspended,
    Removed,
}

impl fmt::Display for MembershipState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invited => write!(f, "invited"),
            Self::Active => write!(f, "active"),
            Self::Suspended => write!(f, "suspended"),
            Self::Removed => write!(f, "removed"),
        }
    }
}

impl FromStr for MembershipState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "invited" => Ok(Self::Invited),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "removed" => Ok(Self::Removed),
            _ => Err(format!("unknown membership state: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuspensionSource {
    Admin,
    Blocklist { scope: String },
}

#[derive(Debug, Clone)]
pub enum MembershipTransition {
    /// invited → active (first successful auth)
    Activate,
    /// active → suspended
    Suspend {
        reason: String,
        source: SuspensionSource,
    },
    /// suspended → active
    Reinstate,
    /// active|suspended → removed
    Remove,
    /// invited → removed (invite expired before first auth)
    Expire,
    /// active → suspended (blocklist hit)
    BlocklistHit { scope: String },
    /// suspended → active (only if blocklist-sourced)
    BlocklistLift,
    /// any non-removed → removed (key loss recovery, creates new grant)
    Replace { new_pubkey: PublicKey },
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum TransitionError {
    #[error("cannot transition from {from}: {reason}")]
    InvalidTransition {
        from: MembershipState,
        reason: String,
    },
    #[error("removed is a terminal state")]
    TerminalState,
}

/// Result of applying a transition. Includes the suspension source when in
/// Suspended state, so we can enforce BlocklistLift constraints.
#[derive(Debug, Clone)]
pub struct StateWithContext {
    pub state: MembershipState,
    pub suspension_source: Option<SuspensionSource>,
}

impl StateWithContext {
    pub fn new(state: MembershipState) -> Self {
        Self {
            state,
            suspension_source: None,
        }
    }

    pub fn suspended(source: SuspensionSource) -> Self {
        Self {
            state: MembershipState::Suspended,
            suspension_source: Some(source),
        }
    }

    pub fn apply(&self, transition: MembershipTransition) -> Result<Self, TransitionError> {
        use MembershipState::*;
        use MembershipTransition::*;

        if self.state == Removed {
            return Err(TransitionError::TerminalState);
        }

        match (&self.state, transition) {
            (Invited, Activate) => Ok(Self::new(Active)),
            (Invited, Expire) => Ok(Self::new(Removed)),
            (Invited, Replace { .. }) => Ok(Self::new(Removed)),

            (Active, Suspend { reason: _, source }) => Ok(Self::suspended(source)),
            (Active, BlocklistHit { scope }) => {
                Ok(Self::suspended(SuspensionSource::Blocklist { scope }))
            }
            (Active, Remove) => Ok(Self::new(Removed)),
            (Active, Replace { .. }) => Ok(Self::new(Removed)),

            (Suspended, Reinstate) => Ok(Self::new(Active)),
            (Suspended, BlocklistLift) => match &self.suspension_source {
                Some(SuspensionSource::Blocklist { .. }) => Ok(Self::new(Active)),
                _ => Err(TransitionError::InvalidTransition {
                    from: Suspended,
                    reason: "blocklist lift only applies to blocklist-sourced suspensions"
                        .to_string(),
                }),
            },
            (Suspended, Remove) => Ok(Self::new(Removed)),
            (Suspended, Replace { .. }) => Ok(Self::new(Removed)),

            (from, transition) => Err(TransitionError::InvalidTransition {
                from: *from,
                reason: format!("{transition:?} not valid from {from}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(state: MembershipState) -> StateWithContext {
        StateWithContext::new(state)
    }

    fn suspended_by_admin() -> StateWithContext {
        StateWithContext::suspended(SuspensionSource::Admin)
    }

    fn suspended_by_blocklist() -> StateWithContext {
        StateWithContext::suspended(SuspensionSource::Blocklist {
            scope: "global".to_string(),
        })
    }

    // --- Valid transitions ---

    #[test]
    fn invited_activate() {
        let result = ctx(MembershipState::Invited)
            .apply(MembershipTransition::Activate)
            .unwrap();
        assert_eq!(result.state, MembershipState::Active);
    }

    #[test]
    fn invited_expire() {
        let result = ctx(MembershipState::Invited)
            .apply(MembershipTransition::Expire)
            .unwrap();
        assert_eq!(result.state, MembershipState::Removed);
    }

    #[test]
    fn active_suspend() {
        let result = ctx(MembershipState::Active)
            .apply(MembershipTransition::Suspend {
                reason: "test".to_string(),
                source: SuspensionSource::Admin,
            })
            .unwrap();
        assert_eq!(result.state, MembershipState::Suspended);
    }

    #[test]
    fn active_remove() {
        let result = ctx(MembershipState::Active)
            .apply(MembershipTransition::Remove)
            .unwrap();
        assert_eq!(result.state, MembershipState::Removed);
    }

    #[test]
    fn active_blocklist_hit() {
        let result = ctx(MembershipState::Active)
            .apply(MembershipTransition::BlocklistHit {
                scope: "org:acme".to_string(),
            })
            .unwrap();
        assert_eq!(result.state, MembershipState::Suspended);
        assert!(matches!(
            result.suspension_source,
            Some(SuspensionSource::Blocklist { .. })
        ));
    }

    #[test]
    fn suspended_reinstate() {
        let result = suspended_by_admin()
            .apply(MembershipTransition::Reinstate)
            .unwrap();
        assert_eq!(result.state, MembershipState::Active);
    }

    #[test]
    fn suspended_remove() {
        let result = suspended_by_admin()
            .apply(MembershipTransition::Remove)
            .unwrap();
        assert_eq!(result.state, MembershipState::Removed);
    }

    #[test]
    fn suspended_blocklist_lift() {
        let result = suspended_by_blocklist()
            .apply(MembershipTransition::BlocklistLift)
            .unwrap();
        assert_eq!(result.state, MembershipState::Active);
    }

    #[test]
    fn replace_from_any_non_removed() {
        let pk = PublicKey::from_bytes([99u8; 32]);
        for state in [
            MembershipState::Invited,
            MembershipState::Active,
            MembershipState::Suspended,
        ] {
            let ctx = if state == MembershipState::Suspended {
                suspended_by_admin()
            } else {
                StateWithContext::new(state)
            };
            let result = ctx
                .apply(MembershipTransition::Replace { new_pubkey: pk })
                .unwrap();
            assert_eq!(result.state, MembershipState::Removed);
        }
    }

    // --- Invalid transitions ---

    #[test]
    fn invited_suspend_fails() {
        let result = ctx(MembershipState::Invited).apply(MembershipTransition::Suspend {
            reason: "nope".to_string(),
            source: SuspensionSource::Admin,
        });
        assert!(result.is_err());
    }

    #[test]
    fn active_activate_fails() {
        let result = ctx(MembershipState::Active).apply(MembershipTransition::Activate);
        assert!(result.is_err());
    }

    #[test]
    fn active_reinstate_fails() {
        let result = ctx(MembershipState::Active).apply(MembershipTransition::Reinstate);
        assert!(result.is_err());
    }

    #[test]
    fn suspended_admin_blocklist_lift_fails() {
        let result = suspended_by_admin().apply(MembershipTransition::BlocklistLift);
        assert!(result.is_err());
    }

    #[test]
    fn removed_is_terminal() {
        let transitions = vec![
            MembershipTransition::Activate,
            MembershipTransition::Reinstate,
            MembershipTransition::Remove,
            MembershipTransition::Expire,
            MembershipTransition::BlocklistLift,
        ];
        for t in transitions {
            let result = ctx(MembershipState::Removed).apply(t);
            assert!(result.is_err(), "removed should be terminal");
        }
    }

    // --- Serde ---

    #[test]
    fn state_serde_roundtrip() {
        for state in [
            MembershipState::Invited,
            MembershipState::Active,
            MembershipState::Suspended,
            MembershipState::Removed,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: MembershipState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn state_display_fromstr() {
        for state in [
            MembershipState::Invited,
            MembershipState::Active,
            MembershipState::Suspended,
            MembershipState::Removed,
        ] {
            let s = state.to_string();
            let back: MembershipState = s.parse().unwrap();
            assert_eq!(state, back);
        }
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn any_transition() -> MembershipTransition {
        let choice: u8 = kani::any();
        kani::assume(choice < 8);
        match choice {
            0 => MembershipTransition::Activate,
            1 => MembershipTransition::Suspend {
                reason: String::new(),
                source: SuspensionSource::Admin,
            },
            2 => MembershipTransition::Reinstate,
            3 => MembershipTransition::Remove,
            4 => MembershipTransition::Expire,
            5 => MembershipTransition::BlocklistHit {
                scope: String::new(),
            },
            6 => MembershipTransition::BlocklistLift,
            7 => MembershipTransition::Replace {
                new_pubkey: PublicKey::from_bytes(kani::any()),
            },
            _ => unreachable!(),
        }
    }

    fn any_state_with_context() -> StateWithContext {
        let choice: u8 = kani::any();
        kani::assume(choice < 5);
        match choice {
            0 => StateWithContext::new(MembershipState::Invited),
            1 => StateWithContext::new(MembershipState::Active),
            2 => StateWithContext::suspended(SuspensionSource::Admin),
            3 => StateWithContext::suspended(SuspensionSource::Blocklist {
                scope: String::new(),
            }),
            4 => StateWithContext::new(MembershipState::Removed),
            _ => unreachable!(),
        }
    }

    /// Prove: no transition from Removed ever succeeds.
    #[kani::proof]
    fn removed_is_terminal() {
        let ctx = StateWithContext::new(MembershipState::Removed);
        let transition = any_transition();
        assert!(ctx.apply(transition).is_err());
    }

    /// Prove: every successful transition produces a valid state, and
    /// suspended states always carry a suspension source.
    #[kani::proof]
    fn valid_transitions_produce_valid_states() {
        let ctx = any_state_with_context();
        let transition = any_transition();
        if let Ok(result) = ctx.apply(transition) {
            assert!(matches!(
                result.state,
                MembershipState::Invited
                    | MembershipState::Active
                    | MembershipState::Suspended
                    | MembershipState::Removed
            ));
            if result.state == MembershipState::Suspended {
                assert!(result.suspension_source.is_some());
            }
        }
    }

    /// Prove: after any sequence of 5 transitions, invariants hold.
    /// Once Removed is reached, all subsequent transitions fail.
    #[kani::proof]
    #[kani::unwind(7)]
    fn multi_step_invariants() {
        let mut ctx = any_state_with_context();
        let mut reached_removed = false;

        for _ in 0..5 {
            let transition = any_transition();
            match ctx.apply(transition) {
                Ok(new_ctx) => {
                    // Must not have been in Removed (terminal)
                    assert!(!reached_removed);
                    if new_ctx.state == MembershipState::Suspended {
                        assert!(new_ctx.suspension_source.is_some());
                    }
                    if new_ctx.state == MembershipState::Removed {
                        reached_removed = true;
                    }
                    ctx = new_ctx;
                }
                Err(_) => {
                    // If we already reached Removed, this must always error
                    if reached_removed {
                        assert!(ctx.state == MembershipState::Removed);
                    }
                }
            }
        }
    }
}
