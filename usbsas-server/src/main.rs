use std::io;

fn main() -> io::Result<()> {
    let matches = clap::Command::new("usbsas-server")
        .arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
                .help("Path of the configuration file")
                .takes_value(true)
                .required(false)
                .default_value(usbsas_utils::USBSAS_CONFIG),
        )
        .arg(
            clap::Arg::new("bind_addr")
                .short('a')
                .long("addr")
                .help("IP address to listen to")
                .takes_value(true)
                .required(false)
                .default_value("127.0.0.1"),
        )
        .arg(
            clap::Arg::new("bind_port")
                .short('p')
                .long("port")
                .help("Port to listen on")
                .takes_value(true)
                .required(false)
                .default_value("8080"),
        )
        .get_matches();

    let config_path = matches.get_one::<String>("config").unwrap().to_string();
    let ip = matches.get_one("bind_addr").unwrap();
    let port = matches.get_one("bind_port").unwrap();

    usbsas_server::server::start_server(config_path, ip, port)
}
