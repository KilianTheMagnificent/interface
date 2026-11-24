//! Which socket a relay lets go when a new `/v5/mac` socket arrives. Shared by
//! the Fly relay (`relay.rs`) and the Cloudflare Worker, which compiles this
//! file by path, so both relays keep the same rule.
//!
//! A socket that has not proved the Mac's private key yet is only waiting. It
//! never takes the place of a proven Mac: waiting sockets have their own room,
//! [`WAITING_SOCKETS`] in all and [`WAITING_PER_ADDRESS`] per client address.
//! A new socket is never refused for lack of room. It displaces the oldest
//! waiting socket from its own address when that address is full, otherwise
//! the oldest waiting socket of all when the room is full. Either way a socket
//! that has not even named an allowed Mac goes before one that has, so a
//! stranger who does not know the Mac's public key can only ever displace
//! other strangers, and someone holding sockets from one address only ever
//! displaces their own.
//!
//! A real Mac opens one socket at a time, names itself in its first message
//! and proves its key within a few round trips; a reconnect after sleep, a
//! network change or a relay restart is one new socket. The limits leave room
//! for a Mac retrying from the same address while its old attempts are still
//! timing out (it waits at least 3 s between attempts and each one is given at
//! most 10 s).

/// Waiting sockets a relay keeps in all, across every client address.
pub const WAITING_SOCKETS: usize = 16;
/// Waiting sockets one client address may hold.
pub const WAITING_PER_ADDRESS: usize = 4;

/// One waiting socket, oldest first in the slice it is listed in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Waiting<'a> {
    /// The client address the relay saw, when it knows one.
    pub address: Option<&'a str>,
    /// Whether its first message named a Mac this relay serves.
    pub named: bool,
}

/// The index, in `waiting` (oldest first), of the socket a new arrival from
/// `address` displaces, or `None` when there is room for it as it is.
#[must_use]
pub fn displaced(waiting: &[Waiting<'_>], address: Option<&str>) -> Option<usize> {
    let oldest = |among: &dyn Fn(&Waiting<'_>) -> bool| {
        waiting
            .iter()
            .position(|socket| among(socket) && !socket.named)
            .or_else(|| waiting.iter().position(among))
    };
    if let Some(address) = address {
        let same = |socket: &Waiting<'_>| socket.address == Some(address);
        if waiting.iter().filter(|socket| same(socket)).count() >= WAITING_PER_ADDRESS {
            return oldest(&same);
        }
    }
    if waiting.len() >= WAITING_SOCKETS {
        return oldest(&|_| true);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(address: &str, named: bool) -> Waiting<'_> {
        Waiting {
            address: Some(address),
            named,
        }
    }

    #[test]
    fn a_full_address_displaces_only_its_own_oldest_stranger() {
        let mut waiting = vec![at("mac", true)];
        waiting.extend((0..WAITING_PER_ADDRESS).map(|_| at("stranger", false)));
        // The stranger's next socket replaces its own oldest, never the Mac's.
        assert_eq!(displaced(&waiting, Some("stranger")), Some(1));
        // The Mac's address has room.
        assert_eq!(displaced(&waiting, Some("mac")), None);
    }

    #[test]
    fn within_one_address_a_socket_that_named_the_mac_goes_last() {
        // A Mac behind the same address as a stranger (a shared NAT).
        let waiting = [
            at("shared", true),
            at("shared", false),
            at("shared", false),
            at("shared", false),
        ];
        assert_eq!(displaced(&waiting, Some("shared")), Some(1));
        let named = [at("shared", true); WAITING_PER_ADDRESS];
        assert_eq!(displaced(&named, Some("shared")), Some(0));
    }

    #[test]
    fn a_full_room_displaces_the_oldest_stranger_of_all() {
        let addresses: Vec<String> = (0..WAITING_SOCKETS)
            .map(|n| format!("10.0.0.{n}"))
            .collect();
        let mut waiting: Vec<Waiting<'_>> = addresses.iter().map(|a| at(a, false)).collect();
        waiting[0].named = true;
        assert_eq!(displaced(&waiting, Some("10.0.1.1")), Some(1));
        assert_eq!(displaced(&waiting, None), Some(1));
        waiting.pop();
        assert_eq!(displaced(&waiting, Some("10.0.1.1")), None);
    }

    #[test]
    fn without_addresses_only_the_whole_room_counts() {
        let waiting = vec![
            Waiting {
                address: None,
                named: false,
            };
            WAITING_SOCKETS - 1
        ];
        assert_eq!(displaced(&waiting, None), None);
        let full = vec![
            Waiting {
                address: None,
                named: false,
            };
            WAITING_SOCKETS
        ];
        assert_eq!(displaced(&full, None), Some(0));
    }

    #[test]
    fn an_empty_relay_admits_anyone() {
        assert_eq!(displaced(&[], Some("mac")), None);
        assert_eq!(displaced(&[], None), None);
    }
}
