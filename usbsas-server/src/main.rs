use std::io;

fn main() -> io::Result<()> {
    let matches = clap::Command::new("usbsas-server")
        .arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
                .help("Path of the configuration file")
                .takes_value(true)
                .required(false),
        )
        .get_matches();
    let config_path = matches
        .value_of("config")
        .unwrap_or(usbsas_utils::USBSAS_CONFIG)
        .to_owned();
    usbsas_server::server::start_server(config_path)
}
