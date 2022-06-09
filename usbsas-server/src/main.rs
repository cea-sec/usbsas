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

    let config_path = matches.value_of("config").unwrap().to_owned();
    let ip = matches.value_of("bind_addr").unwrap();
    let port = matches.value_of("bind_port").unwrap();

    usbsas_server::server::start_server(config_path, ip, port)
}
