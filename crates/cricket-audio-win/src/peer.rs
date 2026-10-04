//! Which process is on the other end of a localhost connection.
//!
//! The browser extension reaches Cricket over a WebSocket on 127.0.0.1. The
//! TCP table lists every connection with its owning process, so the peer's
//! address is enough to learn which browser (Chrome, Edge, Brave...) the
//! extension runs in. That is more reliable than the extension guessing from
//! its user agent, which cannot tell Chromium forks apart.

use std::net::{IpAddr, SocketAddr};

use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    TCP_TABLE_OWNER_PID_CONNECTIONS,
};
use windows::Win32::Networking::WinSock::AF_INET;

use cricket_core::audio::AppId;

use crate::sessions::process_name;

/// Executable of the process that owns the client end of a connection to
/// `server_port` on this machine, where `peer` is the client's address as
/// the server saw it. `None` if it cannot be worked out (IPv6 peer, the
/// connection already closed, or a protected process).
pub fn client_process(peer: SocketAddr, server_port: u16) -> Option<AppId> {
    let IpAddr::V4(peer_ip) = peer.ip() else {
        return None;
    };
    let pid = owning_pid(u32::from(peer_ip).to_be(), peer.port(), server_port)?;
    process_name(pid)
}

/// `local_addr` is in network byte order, as the table stores it.
fn owning_pid(local_addr: u32, local_port: u16, remote_port: u16) -> Option<u32> {
    let rows = tcp_rows()?;
    rows.into_iter()
        .find(|row| {
            row.dwLocalAddr == local_addr
                && port(row.dwLocalPort) == local_port
                && port(row.dwRemotePort) == remote_port
        })
        .map(|row| row.dwOwningPid)
}

/// The table keeps a port in network byte order in the low 16 bits.
fn port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

fn tcp_rows() -> Option<Vec<MIB_TCPROW_OWNER_PID>> {
    // The table can grow between the size query and the read; retry.
    let mut size = 0u32;
    for _ in 0..4 {
        // A u32 buffer keeps the 4-byte alignment the structs need.
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        let result = unsafe {
            GetExtendedTcpTable(
                (size > 0).then(|| buffer.as_mut_ptr().cast()),
                &mut size,
                false,
                AF_INET.0 as u32,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER.0 {
            continue;
        }
        if result != 0 {
            return None;
        }
        let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
        // SAFETY: on success the buffer holds `dwNumEntries` rows, laid out
        // from the `table` field on.
        return Some(unsafe {
            let count = (*table).dwNumEntries as usize;
            std::slice::from_raw_parts((*table).table.as_ptr(), count).to_vec()
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use super::*;

    #[test]
    fn a_port_is_read_from_network_byte_order() {
        // 47835 = 0xBADB; the table holds the bytes BA DB, which read as a
        // little-endian number is 0xDBBA.
        assert_eq!(port(0xDBBA), 47_835);
    }

    #[test]
    fn finds_this_process_as_the_owner_of_a_local_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_port = listener.local_addr().unwrap().port();
        let _client = TcpStream::connect(("127.0.0.1", server_port)).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let peer = accepted.peer_addr().unwrap();

        let found = client_process(peer, server_port).expect("a process");

        let this = std::env::current_exe().unwrap();
        let this = this.file_name().unwrap().to_str().unwrap();
        assert_eq!(found, AppId::new(this));
    }

    #[test]
    fn an_unknown_connection_has_no_owner() {
        let peer: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(client_process(peer, 2), None);
    }
}
