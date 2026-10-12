use crate::Result;
use std::{mem::size_of, net::Ipv4Addr, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR},
    NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
    Networking::WinSock::{AF_INET, IpDadStatePreferred, SOCKADDR_IN},
};

#[derive(Debug, Clone)]
pub struct LocalAddress {
    pub ip: Ipv4Addr,
    pub adapter: String,
    gateway: bool,
    metric: u32,
}

/// Enumerate assigned IPv4 addresses on connected adapters without DNS or
/// external traffic. u64 storage provides the alignment required by IP Helper.
pub fn local_addresses() -> Result<Vec<LocalAddress>> {
    let mut bytes = 15_000u32;
    for _ in 0..3 {
        if bytes > 1024 * 1024 {
            return Err("网卡信息过大".into());
        }
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let status = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST
                    | GAA_FLAG_SKIP_MULTICAST
                    | GAA_FLAG_SKIP_DNS_SERVER
                    | GAA_FLAG_INCLUDE_GATEWAYS,
                null_mut(),
                first,
                &mut bytes,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if status == ERROR_NO_DATA {
            return Ok(vec![]);
        }
        if status != NO_ERROR {
            return Err(format!("读取网卡 IPv4 地址失败（Windows 错误 {status}）"));
        }
        let mut addresses = vec![];
        unsafe {
            let mut current = first;
            while !current.is_null() {
                let adapter = &*current;
                if adapter.OperStatus == IfOperStatusUp {
                    let name = if adapter.FriendlyName.is_null() {
                        String::new()
                    } else {
                        let mut len = 0;
                        while len < 1024 && *adapter.FriendlyName.add(len) != 0 {
                            len += 1;
                        }
                        String::from_utf16_lossy(std::slice::from_raw_parts(
                            adapter.FriendlyName,
                            len,
                        ))
                    };
                    let mut unicast = adapter.FirstUnicastAddress;
                    while !unicast.is_null() {
                        let address = &*unicast;
                        let socket = address.Address.lpSockaddr;
                        if !socket.is_null()
                            && address.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                            && (*socket).sa_family == AF_INET
                            && address.DadState == IpDadStatePreferred
                        {
                            let raw = &*socket.cast::<SOCKADDR_IN>();
                            let octets = raw.sin_addr.S_un.S_un_b;
                            let ip =
                                Ipv4Addr::new(octets.s_b1, octets.s_b2, octets.s_b3, octets.s_b4);
                            if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast() {
                                addresses.push(LocalAddress {
                                    ip,
                                    adapter: name.clone(),
                                    gateway: !adapter.FirstGatewayAddress.is_null(),
                                    metric: adapter.Ipv4Metric,
                                });
                            }
                        }
                        unicast = address.Next;
                    }
                }
                current = adapter.Next;
            }
        }
        addresses.sort_by_key(|a| (a.ip.is_link_local(), !a.gateway, a.metric, a.ip));
        let mut seen = std::collections::HashSet::new();
        addresses.retain(|a| seen.insert(a.ip));
        return Ok(addresses);
    }
    Err("网卡信息正在变化，请点击刷新后重试".into())
}

pub fn valid_listen_address(value: &str) -> bool {
    value.parse::<Ipv4Addr>().is_ok_and(|ip| {
        !ip.is_multicast() && !ip.is_broadcast() && (ip.is_unspecified() || ip.octets()[0] != 0)
    })
}

pub fn preferred_address() -> String {
    local_addresses()
        .ok()
        .and_then(|v| v.first().map(|a| a.ip.to_string()))
        .unwrap_or_else(|| "127.0.0.1".into())
}
