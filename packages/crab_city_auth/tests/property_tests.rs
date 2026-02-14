use proptest::prelude::*;

use crab_city_auth::capability::Capability;
use crab_city_auth::invite::{Invite, InviteLink};
use crab_city_auth::keys::{PublicKey, SigningKey};
use crab_city_auth::membership::{
    MembershipState, MembershipTransition, StateWithContext, SuspensionSource,
};
use crab_city_auth::noun::IdentityNoun;

// --- Capability algebra ---

fn arb_capability() -> impl Strategy<Value = Capability> {
    prop_oneof![
        Just(Capability::View),
        Just(Capability::Collaborate),
        Just(Capability::Admin),
        Just(Capability::Owner),
    ]
}

proptest! {
    #[test]
    fn intersect_commutative(a in arb_capability(), b in arb_capability()) {
        let ar = a.access_rights();
        let br = b.access_rights();
        let ab = ar.intersect(&br);
        let ba = br.intersect(&ar);
        prop_assert_eq!(format!("{ab:?}"), format!("{ba:?}"));
    }

    #[test]
    fn intersect_idempotent(cap in arb_capability()) {
        let r = cap.access_rights();
        let rr = r.intersect(&r);
        prop_assert_eq!(format!("{r:?}"), format!("{rr:?}"));
    }

    #[test]
    fn from_access_roundtrip(cap in arb_capability()) {
        let access = cap.access_rights();
        prop_assert_eq!(Capability::from_access(&access), Some(cap));
    }

    #[test]
    fn intersect_narrows(a in arb_capability(), b in arb_capability()) {
        let ar = a.access_rights();
        let br = b.access_rights();
        let intersection = ar.intersect(&br);
        // Intersection must be a subset of both inputs
        prop_assert!(ar.is_superset_of(&intersection));
        prop_assert!(br.is_superset_of(&intersection));
    }
}

// --- Invite + Capability interaction ---

/// Helper: create a delegatable invite (max_depth > 0) by constructing the root
/// link manually, since `create_flat` always uses max_depth=0.
fn create_delegatable(
    sk: &SigningKey,
    instance: &PublicKey,
    capability: Capability,
    max_uses: u32,
    max_depth: u8,
) -> Invite {
    let mut rng = rand::thread_rng();
    let genesis_prev = [0u8; 32];
    let link = InviteLink::sign(
        sk,
        &genesis_prev,
        instance,
        capability,
        max_depth,
        max_uses,
        None,
        &mut rng,
    );
    Invite {
        version: 0x01,
        instance: *instance,
        links: vec![link],
    }
}

#[test]
fn delegated_capability_never_exceeds_root() {
    let mut rng = rand::thread_rng();
    let sk_root = SigningKey::generate(&mut rng);
    let sk_del = SigningKey::generate(&mut rng);
    let instance = PublicKey::from_bytes([1u8; 32]);

    for root_cap in [
        Capability::View,
        Capability::Collaborate,
        Capability::Admin,
        Capability::Owner,
    ] {
        // Create with max_depth=3 so delegation is allowed
        let invite = create_delegatable(&sk_root, &instance, root_cap, 10, 3);
        let claims = invite.verify().unwrap();
        assert_eq!(claims.capability, root_cap);

        // Delegate with same cap — should work
        let delegated = Invite::delegate(&invite, &sk_del, root_cap, 5, None, &mut rng).unwrap();
        let del_claims = delegated.verify().unwrap();
        assert_eq!(del_claims.capability, root_cap);

        // The effective capability should always be <= root
        let root_rights = root_cap.access_rights();
        let del_rights = del_claims.capability.access_rights();
        assert!(root_rights.is_superset_of(&del_rights));
    }
}

#[test]
fn delegation_chain_capabilities_only_narrow() {
    let mut rng = rand::thread_rng();
    let sk1 = SigningKey::generate(&mut rng);
    let sk2 = SigningKey::generate(&mut rng);
    let sk3 = SigningKey::generate(&mut rng);
    let instance = PublicKey::from_bytes([2u8; 32]);

    // Owner → Admin → Collaborate (3-hop narrowing chain)
    let inv1 = create_delegatable(&sk1, &instance, Capability::Owner, 10, 3);
    let inv2 = Invite::delegate(&inv1, &sk2, Capability::Admin, 5, None, &mut rng).unwrap();
    let inv3 = Invite::delegate(&inv2, &sk3, Capability::Collaborate, 5, None, &mut rng).unwrap();

    let claims = inv3.verify().unwrap();
    assert_eq!(claims.capability, Capability::Collaborate);

    // Same level is OK
    assert!(
        Invite::delegate(&inv1, &sk2, Capability::Owner, 5, None, &mut rng)
            .unwrap()
            .verify()
            .is_ok()
    );
    // But we can't go *up* — admin parent can't produce owner child
    let admin_inv = Invite::delegate(&inv1, &sk2, Capability::Admin, 5, None, &mut rng).unwrap();
    assert!(Invite::delegate(&admin_inv, &sk3, Capability::Owner, 5, None, &mut rng).is_err());
}

// --- Membership state machine exhaustive exploration ---

#[test]
fn all_valid_transitions_produce_valid_states() {
    let all_states = [
        MembershipState::Invited,
        MembershipState::Active,
        MembershipState::Suspended,
        MembershipState::Removed,
    ];

    let pk = PublicKey::from_bytes([99u8; 32]);

    let transitions: Vec<MembershipTransition> = vec![
        MembershipTransition::Activate,
        MembershipTransition::Suspend {
            reason: "test".to_string(),
            source: SuspensionSource::Admin,
        },
        MembershipTransition::Reinstate,
        MembershipTransition::Remove,
        MembershipTransition::Expire,
        MembershipTransition::BlocklistHit {
            scope: "global".to_string(),
        },
        MembershipTransition::BlocklistLift,
        MembershipTransition::Replace { new_pubkey: pk },
    ];

    for state in all_states {
        for transition in &transitions {
            let ctx = if state == MembershipState::Suspended {
                StateWithContext::suspended(SuspensionSource::Admin)
            } else {
                StateWithContext::new(state)
            };

            match ctx.apply(transition.clone()) {
                Ok(result) => {
                    assert!(
                        [
                            MembershipState::Invited,
                            MembershipState::Active,
                            MembershipState::Suspended,
                            MembershipState::Removed,
                        ]
                        .contains(&result.state),
                        "transition from {state} via {transition:?} produced invalid state"
                    );
                }
                Err(_) => {}
            }
        }
    }
}

#[test]
fn removed_is_always_terminal() {
    let pk = PublicKey::from_bytes([99u8; 32]);

    let transitions: Vec<MembershipTransition> = vec![
        MembershipTransition::Activate,
        MembershipTransition::Suspend {
            reason: "test".to_string(),
            source: SuspensionSource::Admin,
        },
        MembershipTransition::Reinstate,
        MembershipTransition::Remove,
        MembershipTransition::Expire,
        MembershipTransition::BlocklistHit {
            scope: "x".to_string(),
        },
        MembershipTransition::BlocklistLift,
        MembershipTransition::Replace { new_pubkey: pk },
    ];

    let ctx = StateWithContext::new(MembershipState::Removed);
    for t in transitions {
        assert!(ctx.apply(t.clone()).is_err(), "removed should reject {t:?}");
    }
}

// --- Noun round-trip ---

proptest! {
    #[test]
    fn handle_roundtrip(s in "[a-z][a-z0-9\\-]{2,28}[a-z0-9]") {
        if !s.starts_with('-') && !s.ends_with('-') {
            let noun_str = format!("@{s}");
            if let Ok(noun) = noun_str.parse::<IdentityNoun>() {
                let rendered = noun.to_string();
                let reparsed: IdentityNoun = rendered.parse().unwrap();
                prop_assert_eq!(noun, reparsed);
            }
        }
    }

    #[test]
    fn github_roundtrip(s in "[a-zA-Z][a-zA-Z0-9\\-]{0,38}") {
        if !s.starts_with('-') {
            let noun_str = format!("github:{s}");
            if let Ok(noun) = noun_str.parse::<IdentityNoun>() {
                let rendered = noun.to_string();
                let reparsed: IdentityNoun = rendered.parse().unwrap();
                prop_assert_eq!(noun, reparsed);
            }
        }
    }
}

// --- diff consistent with intersect ---

#[test]
fn diff_and_intersect_consistent() {
    let old = Capability::View.access_rights();
    let new = Capability::Collaborate.access_rights();

    let (added, _removed) = old.diff(&new);
    let intersection = old.intersect(&new);

    // Everything in `new` should be in either `added` or `intersection`
    for right in &new.0 {
        for action in &right.actions {
            let in_added = added.contains(&right.type_, action);
            let in_intersection = intersection.contains(&right.type_, action);
            assert!(
                in_added || in_intersection,
                "{}:{} missing from both added and intersection",
                right.type_,
                action
            );
        }
    }
}
