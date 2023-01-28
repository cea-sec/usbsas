use serde::Serialize;
use systemstat::{saturating_sub_bytes, Platform, System};

#[derive(Serialize, Debug)]
struct StatMount {
    fs_mounted_from: String,
    fs_type: String,
    fs_mounted_on: String,
    size_avail: String,
    size_total: String,
}

#[derive(Serialize, Debug)]
struct StatNet {
    name: String,
    addrs: String,
    mac: String,
}

#[derive(Serialize, Debug)]
struct StatMem {
    used: String,
    total: String,
}

#[derive(Serialize, Debug)]
struct StatLoad {
    one: String,
    five: String,
    fifteen: String,
}

#[derive(Serialize, Debug)]
pub(crate) struct ServerInfos {
    mount: Vec<StatMount>,
    network: Vec<StatNet>,
    memory: StatMem,
    load: StatLoad,
    time: String,
}

pub(crate) fn get_server_infos() -> ServerInfos {
    let sys = System::new();
    let mount_stats = match sys.mounts() {
        Ok(mounts) => {
            let mut mount_stats = vec![];
            for mount in mounts.iter() {
                mount_stats.push(StatMount {
                    fs_mounted_from: mount.fs_mounted_from.to_owned(),
                    fs_type: mount.fs_type.to_owned(),
                    fs_mounted_on: mount.fs_mounted_on.to_owned(),
                    size_avail: mount.avail.to_string(),
                    size_total: mount.total.to_string(),
                });
            }
            mount_stats.sort_by(|a, b| a.fs_mounted_on.partial_cmp(&b.fs_mounted_on).unwrap());
            mount_stats
        }
        _ => vec![],
    };

    let networks = match sys.networks() {
        Ok(netifs) => {
            let mut networks = vec![];
            for netif in netifs.values() {
                let addrs_str: Vec<String> = netif
                    .addrs
                    .iter()
                    .map(|addr| format!("{:?}", addr.addr))
                    .collect();

                let mac_str = match mac_address::mac_address_by_name(&netif.name) {
                    Ok(mac) => match mac {
                        Some(mac) => format!("{mac}"),
                        None => String::default(),
                    },
                    Err(_) => String::default(),
                };

                let network = StatNet {
                    name: netif.name.to_owned(),
                    addrs: addrs_str.join(", "),
                    mac: mac_str,
                };
                networks.push(network);
            }
            networks
        }
        _ => vec![],
    };

    let memory = match sys.memory() {
        Ok(mem) => StatMem {
            used: saturating_sub_bytes(mem.total, mem.free).to_string(),
            total: mem.total.to_string(),
        },
        _ => StatMem {
            used: "Unk".to_owned(),
            total: "Unk".to_owned(),
        },
    };

    let load = match sys.load_average() {
        Ok(loadavg) => StatLoad {
            one: loadavg.one.to_string(),
            five: loadavg.five.to_string(),
            fifteen: loadavg.fifteen.to_string(),
        },
        _ => StatLoad {
            one: "Unk".to_owned(),
            five: "Unk".to_owned(),
            fifteen: "Unk".to_owned(),
        },
    };

    let time = {
        let datetime = time::OffsetDateTime::now_utc();
        format!(
            "{}/{}/{} {}:{}:{}",
            datetime.day(),
            datetime.month(),
            datetime.year(),
            datetime.hour(),
            datetime.minute(),
            datetime.second(),
        )
    };

    ServerInfos {
        mount: mount_stats,
        network: networks,
        memory,
        load,
        time,
    }
}
