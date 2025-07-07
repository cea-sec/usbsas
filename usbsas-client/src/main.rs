use std::env;
use usbsas_client::{client_clap, GUI};

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(
            "info,wgpu_hal=error,iced_winit=error,iced_wgpu=error,wgpu_core=error",
        ),
    )
    .init();
    let matches = client_clap().get_matches();
    let (width, height) = (
        matches.get_one::<u32>("width").unwrap().to_owned(),
        matches.get_one::<u32>("height").unwrap().to_owned(),
    );

    // see https://github.com/rust-windowing/winit/issues/2231
    if env::var("WINIT_X11_SCALE_FACTOR").is_err() {
        env::set_var("WINIT_X11_SCALE_FACTOR", "1.0")
    };

    iced::application("usbsas", GUI::update, GUI::view)
        .window_size((width as f32, height as f32))
        .subscription(GUI::subscription)
        .run_with(GUI::new)
        .expect("run");
}
