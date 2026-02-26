//! Federation interconnect: instance-to-instance tunnels over iroh.
//!
//! - `protocol` — tunnel message types (Hello/Welcome, Authenticate/AuthResult, UserMessage)
//! - `manager` — outbound ConnectionManager (home side)
//! - `host` — inbound HostHandler (host side)

pub mod host;
pub mod manager;
pub mod protocol;

#[cfg(test)]
mod e2e_tests;

/// Which Crab City the user is currently viewing.
///
/// `Local` means they're looking at their own instance's terminals/chat/tasks.
/// `Remote` means they're viewing a remote Crab City they're connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrabCityContext {
    Local,
    Remote {
        host_node_id: [u8; 32],
        host_name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_local_equality() {
        assert_eq!(CrabCityContext::Local, CrabCityContext::Local);
    }

    #[test]
    fn context_remote_equality() {
        let a = CrabCityContext::Remote {
            host_node_id: [1u8; 32],
            host_name: "Bob's Workshop".into(),
        };
        let b = CrabCityContext::Remote {
            host_node_id: [1u8; 32],
            host_name: "Bob's Workshop".into(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn context_local_ne_remote() {
        let remote = CrabCityContext::Remote {
            host_node_id: [1u8; 32],
            host_name: "Bob's Workshop".into(),
        };
        assert_ne!(CrabCityContext::Local, remote);
    }

    #[test]
    fn context_different_hosts_ne() {
        let a = CrabCityContext::Remote {
            host_node_id: [1u8; 32],
            host_name: "Alice's Lab".into(),
        };
        let b = CrabCityContext::Remote {
            host_node_id: [2u8; 32],
            host_name: "Bob's Workshop".into(),
        };
        assert_ne!(a, b);
    }
}
