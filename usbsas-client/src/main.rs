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
    iced::application("usbsas", GUI::update, GUI::view)
        .window_size((width as f32, height as f32))
        .subscription(GUI::subscription)
        .run_with(GUI::new)
        .expect("run");
}
