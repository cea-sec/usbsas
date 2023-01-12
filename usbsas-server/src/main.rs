use std::io;
use usbsas_utils::clap::UsbsasClap;

fn main() -> io::Result<()> {
    let matches = usbsas_utils::clap::new_usbsas_cmd("usbsas-server")
        .add_config_arg()
        .arg(
            clap::Arg::new("bind_addr")
                .short('a')
                .long("addr")
                .help("IP address to listen to")
                .num_args(1)
                .required(false)
                .default_value("127.0.0.1"),
        )
        .arg(
            clap::Arg::new("bind_port")
                .short('p')
                .long("port")
                .help("Port to listen on")
                .num_args(1)
                .required(false)
                .default_value("8080"),
        )
        .get_matches();

    let config_path = matches.get_one::<String>("config").unwrap().to_string();
    let ip = matches.get_one("bind_addr").unwrap();
    let port = matches.get_one("bind_port").unwrap();

    usbsas_server::server::start_server(config_path, ip, port)
}
