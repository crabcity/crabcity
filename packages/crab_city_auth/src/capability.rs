//! Capability levels and fine-grained access rights.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// --- Capability ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    View,
    Collaborate,
    Admin,
    Owner,
}

impl Capability {
    pub fn access_rights(&self) -> AccessRights {
        let mut rights = Vec::new();

        // View
        rights.push(AccessRight::new("content", &["read"]));
        rights.push(AccessRight::new("terminals", &["read"]));

        if *self >= Capability::Collaborate {
            rights.push(AccessRight::new("terminals", &["input"]));
            rights.push(AccessRight::new("chat", &["send"]));
            rights.push(AccessRight::new("tasks", &["read", "create", "edit"]));
            rights.push(AccessRight::new("instances", &["create"]));
        }

        if *self >= Capability::Admin {
            rights.push(AccessRight::new(
                "members",
                &["read", "invite", "suspend", "reinstate", "remove", "update"],
            ));
        }

        if *self >= Capability::Owner {
            rights.push(AccessRight::new("instance", &["manage", "transfer"]));
        }

        // Merge entries with the same type
        AccessRights::from_merged(rights)
    }

    /// Reverse mapping: if access exactly matches a preset, return it.
    pub fn from_access(access: &AccessRights) -> Option<Capability> {
        for cap in [
            Capability::Owner,
            Capability::Admin,
            Capability::Collaborate,
            Capability::View,
        ] {
            if cap.access_rights() == *access {
                return Some(cap);
            }
        }
        None
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::View => write!(f, "view"),
            Self::Collaborate => write!(f, "collaborate"),
            Self::Admin => write!(f, "admin"),
            Self::Owner => write!(f, "owner"),
        }
    }
}

impl FromStr for Capability {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "view" => Ok(Self::View),
            "collaborate" => Ok(Self::Collaborate),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            _ => Err(format!("unknown capability: {s}")),
        }
    }
}

// --- AccessRight ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRight {
    #[serde(rename = "type")]
    pub type_: String,
    pub actions: Vec<String>,
}

impl AccessRight {
    pub fn new(type_: &str, actions: &[&str]) -> Self {
        let mut actions: Vec<String> = actions.iter().map(|a| a.to_string()).collect();
        actions.sort();
        Self {
            type_: type_.to_string(),
            actions,
        }
    }
}

// --- AccessRights ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccessRights(pub Vec<AccessRight>);

impl AccessRights {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn single(type_: &str, action: &str) -> Self {
        Self(vec![AccessRight::new(type_, &[action])])
    }

    /// Merge multiple AccessRight entries that share the same type.
    fn from_merged(rights: Vec<AccessRight>) -> Self {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in rights {
            map.entry(r.type_).or_default().extend(r.actions);
        }
        let merged = map
            .into_iter()
            .map(|(type_, mut actions)| {
                actions.sort();
                actions.dedup();
                AccessRight { type_, actions }
            })
            .collect();
        Self(merged)
    }

    /// Normalize for comparison: sort types, sort actions within each type.
    fn normalized(&self) -> Vec<AccessRight> {
        let mut rights = self.0.clone();
        for r in &mut rights {
            r.actions.sort();
            r.actions.dedup();
        }
        rights.sort_by(|a, b| a.type_.cmp(&b.type_));
        // Merge same-type entries
        let mut merged: Vec<AccessRight> = Vec::new();
        for r in rights {
            if let Some(last) = merged.last_mut() {
                if last.type_ == r.type_ {
                    last.actions.extend(r.actions);
                    last.actions.sort();
                    last.actions.dedup();
                    continue;
                }
            }
            merged.push(r);
        }
        merged
    }

    /// Intersection: for each type present in both, intersect actions.
    /// Types present in only one side are dropped.
    pub fn intersect(&self, other: &AccessRights) -> AccessRights {
        let a = self.normalized();
        let b = other.normalized();
        let mut result = Vec::new();
        for ar in &a {
            if let Some(br) = b.iter().find(|r| r.type_ == ar.type_) {
                let actions: Vec<String> = ar
                    .actions
                    .iter()
                    .filter(|act| br.actions.contains(act))
                    .cloned()
                    .collect();
                if !actions.is_empty() {
                    result.push(AccessRight {
                        type_: ar.type_.clone(),
                        actions,
                    });
                }
            }
        }
        AccessRights(result)
    }

    /// Does this set contain the given type+action?
    pub fn contains(&self, type_: &str, action: &str) -> bool {
        self.0
            .iter()
            .any(|r| r.type_ == type_ && r.actions.iter().any(|a| a == action))
    }

    /// Is self a superset of other? (every type+action in other exists in self)
    pub fn is_superset_of(&self, other: &AccessRights) -> bool {
        for r in &other.0 {
            for action in &r.actions {
                if !self.contains(&r.type_, action) {
                    return false;
                }
            }
        }
        true
    }

    /// Diff: returns (added, removed) where added = in other but not self,
    /// removed = in self but not other.
    pub fn diff(&self, other: &AccessRights) -> (AccessRights, AccessRights) {
        let a_norm = self.normalized();
        let b_norm = other.normalized();

        let mut added = Vec::new();
        let mut removed = Vec::new();

        // Find all types mentioned in either
        let mut all_types: Vec<&str> = a_norm
            .iter()
            .map(|r| r.type_.as_str())
            .chain(b_norm.iter().map(|r| r.type_.as_str()))
            .collect();
        all_types.sort();
        all_types.dedup();

        for type_ in all_types {
            let a_actions: Vec<&str> = a_norm
                .iter()
                .filter(|r| r.type_ == type_)
                .flat_map(|r| r.actions.iter().map(|s| s.as_str()))
                .collect();
            let b_actions: Vec<&str> = b_norm
                .iter()
                .filter(|r| r.type_ == type_)
                .flat_map(|r| r.actions.iter().map(|s| s.as_str()))
                .collect();

            let add: Vec<String> = b_actions
                .iter()
                .filter(|a| !a_actions.contains(a))
                .map(|a| a.to_string())
                .collect();
            let rem: Vec<String> = a_actions
                .iter()
                .filter(|a| !b_actions.contains(a))
                .map(|a| a.to_string())
                .collect();

            if !add.is_empty() {
                added.push(AccessRight {
                    type_: type_.to_string(),
                    actions: add,
                });
            }
            if !rem.is_empty() {
                removed.push(AccessRight {
                    type_: type_.to_string(),
                    actions: rem,
                });
            }
        }

        (AccessRights(added), AccessRights(removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_ordering() {
        assert!(Capability::View < Capability::Collaborate);
        assert!(Capability::Collaborate < Capability::Admin);
        assert!(Capability::Admin < Capability::Owner);
    }

    #[test]
    fn capability_serde_roundtrip() {
        for cap in [
            Capability::View,
            Capability::Collaborate,
            Capability::Admin,
            Capability::Owner,
        ] {
            let json = serde_json::to_string(&cap).unwrap();
            let back: Capability = serde_json::from_str(&json).unwrap();
            assert_eq!(cap, back);
        }
    }

    #[test]
    fn capability_display_fromstr() {
        for cap in [
            Capability::View,
            Capability::Collaborate,
            Capability::Admin,
            Capability::Owner,
        ] {
            let s = cap.to_string();
            let back: Capability = s.parse().unwrap();
            assert_eq!(cap, back);
        }
    }

    #[test]
    fn preset_superset_ordering() {
        let view = Capability::View.access_rights();
        let collab = Capability::Collaborate.access_rights();
        let admin = Capability::Admin.access_rights();
        let owner = Capability::Owner.access_rights();

        assert!(collab.is_superset_of(&view));
        assert!(admin.is_superset_of(&collab));
        assert!(owner.is_superset_of(&admin));
        assert!(owner.is_superset_of(&view));

        assert!(!view.is_superset_of(&collab));
        assert!(!collab.is_superset_of(&admin));
    }

    #[test]
    fn from_access_roundtrip() {
        for cap in [
            Capability::View,
            Capability::Collaborate,
            Capability::Admin,
            Capability::Owner,
        ] {
            let access = cap.access_rights();
            assert_eq!(Capability::from_access(&access), Some(cap));
        }
    }

    #[test]
    fn intersect_commutative() {
        let a = Capability::Collaborate.access_rights();
        let b = Capability::Admin.access_rights();
        let ab = a.intersect(&b);
        let ba = b.intersect(&a);
        assert_eq!(ab.normalized(), ba.normalized());
    }

    #[test]
    fn intersect_idempotent() {
        let a = Capability::Admin.access_rights();
        let aa = a.intersect(&a);
        assert_eq!(a.normalized(), aa.normalized());
    }

    #[test]
    fn intersect_disjoint_is_empty() {
        let view = Capability::View.access_rights();
        let tasks_only = AccessRights::single("tasks", "create");
        let result = view.intersect(&tasks_only);
        assert!(result.0.is_empty());
    }

    #[test]
    fn contains_basic() {
        let collab = Capability::Collaborate.access_rights();
        assert!(collab.contains("content", "read"));
        assert!(collab.contains("tasks", "create"));
        assert!(!collab.contains("members", "invite"));
    }

    #[test]
    fn contains_consistent_with_superset() {
        let admin = Capability::Admin.access_rights();
        assert!(admin.contains("members", "invite"));
        let single = AccessRights::single("members", "invite");
        assert!(admin.is_superset_of(&single));
    }

    #[test]
    fn diff_basic() {
        let view = Capability::View.access_rights();
        let collab = Capability::Collaborate.access_rights();
        let (added, removed) = view.diff(&collab);
        // collab has more than view, so added should be non-empty
        assert!(!added.0.is_empty());
        // view has nothing that collab doesn't, so removed should be empty
        assert!(removed.0.is_empty());
    }

    #[test]
    fn access_rights_serde_roundtrip() {
        let rights = Capability::Admin.access_rights();
        let json = serde_json::to_string(&rights).unwrap();
        let back: AccessRights = serde_json::from_str(&json).unwrap();
        assert_eq!(rights, back);
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn any_capability() -> Capability {
        let choice: u8 = kani::any();
        kani::assume(choice < 4);
        match choice {
            0 => Capability::View,
            1 => Capability::Collaborate,
            2 => Capability::Admin,
            3 => Capability::Owner,
            _ => unreachable!(),
        }
    }

    /// Prove: intersect is commutative for all capability pairs.
    #[kani::proof]
    fn intersect_is_commutative() {
        let a = any_capability();
        let b = any_capability();
        let ar = a.access_rights();
        let br = b.access_rights();
        let ab = ar.intersect(&br);
        let ba = br.intersect(&ar);
        // Mutual superset ↔ set equality
        assert!(ab.is_superset_of(&ba));
        assert!(ba.is_superset_of(&ab));
    }

    /// Prove: if capability a >= b (by Ord), then a's rights are a
    /// superset of b's rights.
    #[kani::proof]
    fn superset_follows_ord() {
        let a = any_capability();
        let b = any_capability();
        if a >= b {
            assert!(a.access_rights().is_superset_of(&b.access_rights()));
        }
    }

    /// Prove: from_access round-trips for every capability.
    #[kani::proof]
    fn from_access_roundtrips() {
        let cap = any_capability();
        let access = cap.access_rights();
        assert_eq!(Capability::from_access(&access), Some(cap));
    }

    /// Prove: intersect always narrows — the result is a subset of both inputs.
    #[kani::proof]
    fn intersect_narrows() {
        let a = any_capability();
        let b = any_capability();
        let ar = a.access_rights();
        let br = b.access_rights();
        let intersection = ar.intersect(&br);
        assert!(ar.is_superset_of(&intersection));
        assert!(br.is_superset_of(&intersection));
    }
}
