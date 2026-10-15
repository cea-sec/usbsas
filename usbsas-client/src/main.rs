use std::env;
use usbsas_client::{GUI, client_clap};

fn main() -> iced::Result {
    env_logger::init_from_env(
        env_logger::Env::default().default_filter_or(
            "info,wgpu_hal=error,iced_winit=error,iced_wgpu=error,wgpu_core=error,cosmic_text=error,wgpu_hal=error,zbus=error,winit=error,naga=error,sctk=error,tracing=error",
        ),
    );
    let matches = client_clap().get_matches();
    let (width, height) = (
        matches.get_one::<u32>("width").unwrap().to_owned(),
        matches.get_one::<u32>("height").unwrap().to_owned(),
    );

    iced::application(GUI::new, GUI::update, GUI::view)
        .window_size((width as f32, height as f32))
        .subscription(GUI::subscription)
        .run()
}
