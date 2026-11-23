//! Win32 process and TCP tables, the only `unsafe` code in this crate.

use std::mem::{size_of, zeroed};
use std::ptr::{addr_of, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, INVALID_HANDLE_VALUE, NO_ERROR,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

use crate::{Server, descendants};

/// Listening TCP ports owned by any process below `shell_pid`.
pub fn servers(shell_pid: u32) -> Vec<Server> {
    let processes = processes();
    let tree = descendants(
        shell_pid,
        &processes
            .iter()
            .map(|p| (p.pid, p.parent))
            .collect::<Vec<_>>(),
    );
    if tree.is_empty() {
        return Vec::new();
    }
    let mut servers: Vec<Server> = listeners()
        .into_iter()
        .filter(|(_, pid)| tree.contains(pid))
        .filter_map(|(port, pid)| {
            let process = processes.iter().find(|p| p.pid == pid)?;
            Some(Server {
                port,
                name: process.name.clone(),
            })
        })
        .collect();
    servers.sort();
    servers.dedup();
    servers
}

struct Process {
    pid: u32,
    parent: u32,
    name: String,
}

fn processes() -> Vec<Process> {
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is checked, used only here, and closed once.
    // `entry` is a plain C struct with `dwSize` set as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snapshot, &mut entry);
        while ok != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            let name = exe
                .strip_suffix(".exe")
                .or_else(|| exe.strip_suffix(".EXE"))
                .unwrap_or(&exe);
            out.push(Process {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
                name: name.to_owned(),
            });
            ok = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    out
}

/// (port, owning pid) for every listening TCP socket, IPv4 and IPv6.
fn listeners() -> Vec<(u16, u32)> {
    let mut out = Vec::new();
    // Ports are stored in network byte order in the low 16 bits.
    let port = |raw: u32| u16::from_be(raw as u16);
    if let Some(buf) = tcp_table(AF_INET) {
        // SAFETY: GetExtendedTcpTable filled `buf` with a MIB_TCPTABLE_OWNER_PID
        // holding `dwNumEntries` rows; the buffer is u32-aligned.
        unsafe {
            let table = buf.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let rows = addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>();
            for i in 0..(*table).dwNumEntries as usize {
                let row = &*rows.add(i);
                out.push((port(row.dwLocalPort), row.dwOwningPid));
            }
        }
    }
    if let Some(buf) = tcp_table(AF_INET6) {
        // SAFETY: as above, for the IPv6 table layout.
        unsafe {
            let table = buf.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let rows = addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>();
            for i in 0..(*table).dwNumEntries as usize {
                let row = &*rows.add(i);
                out.push((port(row.dwLocalPort), row.dwOwningPid));
            }
        }
    }
    out
}

/// The raw listener table for one address family, in a u32 buffer so the
/// table's fields are aligned.
fn tcp_table(family: u16) -> Option<Vec<u32>> {
    let mut size = 0u32;
    // SAFETY: a null table with a size out-pointer only reports the size.
    let status = unsafe {
        GetExtendedTcpTable(
            null_mut(),
            &mut size,
            0,
            u32::from(family),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if status != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    // Sockets can open between the two calls; leave room for a few more.
    let mut buf = vec![0u32; (size as usize).div_ceil(4) + 64];
    let mut size = (buf.len() * 4) as u32;
    // SAFETY: `buf` is `size` bytes and writable.
    let status = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            u32::from(family),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    (status == NO_ERROR).then_some(buf)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::process::Command;

    use super::*;

    #[test]
    fn finds_a_listener_started_by_a_child() {
        // This test process stands in for the shell: it starts a child that
        // listens, and that child must show up as a server.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let script = format!(
            "$l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, {port}); $l.Start(); Start-Sleep 20"
        );
        let Ok(mut child) = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .spawn()
        else {
            return;
        };

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let found = loop {
            let servers = servers(std::process::id());
            if let Some(server) = servers.into_iter().find(|s| s.port == port) {
                break Some(server);
            }
            if std::time::Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        let _ = child.kill();
        let server = found.expect("listener not found under this process");
        assert_eq!(server.name.to_lowercase(), "powershell");
    }

    #[test]
    fn a_process_with_no_children_has_no_servers() {
        assert!(servers(u32::MAX).is_empty());
    }
}
