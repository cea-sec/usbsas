use std::io;

fn main() -> io::Result<()> {
    usbsas_server::server::start_server()
}
