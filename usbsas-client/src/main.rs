use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::unix::WindowExtUnix,
    window::{Fullscreen, WindowBuilder},
};
use wry::{WebViewBuilder, WebViewBuilderExtUnix};

fn main() -> wry::Result<()> {
    let cmd_matches = clap::Command::new("usbsas-client")
        .arg(
            clap::Arg::new("path")
                .value_name("PATH")
                .help("path of web directory")
                .num_args(1)
                .required(true),
        )
        .arg(
            clap::Arg::new("width")
                .value_name("WIDTH")
                .short('W')
                .long("width")
                .help("window width")
                .num_args(1)
                .default_value("1080")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            clap::Arg::new("height")
                .value_name("HEIGHT")
                .short('H')
                .long("height")
                .help("window height")
                .num_args(1)
                .default_value("900")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            clap::Arg::new("fullscreen")
                .value_name("FULLSCREEN")
                .short('f')
                .long("fullscreen")
                .help("window full screen")
                .num_args(0)
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let web_dir_path =
        std::fs::canonicalize(cmd_matches.get_one::<String>("path").unwrap()).unwrap();
    let web_dir_path = web_dir_path.to_str().unwrap();
    let (width, height) = (
        cmd_matches.get_one::<u32>("width").unwrap().to_owned(),
        cmd_matches.get_one::<u32>("height").unwrap().to_owned(),
    );
    let window_size = LogicalSize::new(width, height);

    usbsas_sandbox::landlock(
        Some(&[
            web_dir_path,
            "/dev",
            "/etc",
            "/home",
            "/lib",
            "/proc",
            "/run",
            "/sys",
            "/usr",
            "/var/cache",
            "/var/lib",
        ]),
        Some(&["/dev/dri"]),
    )
    .expect("couldn't apply landlock ruleset");

    let event_loop = EventLoop::new();
    let mut window_builder = WindowBuilder::new()
        .with_always_on_top(true)
        .with_focused(true)
        .with_decorations(false)
        .with_closable(false)
        .with_inner_size(window_size)
        .with_title("usbsas-kiosk");
    window_builder = if *cmd_matches.get_one::<bool>("fullscreen").unwrap() {
        window_builder.with_fullscreen(Some(Fullscreen::Borderless(None)))
    } else {
        window_builder
    };
    let window = window_builder.build(&event_loop).unwrap();
    let builder = WebViewBuilder::new();
    let webview = builder
        .with_url(format!("file:///{}/index.html", web_dir_path))
        .with_devtools(false)
        .with_focused(true)
        .build_gtk(window.default_vbox().unwrap())?;
    let _ = webview.clear_all_browsing_data();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                println!("usbsas web kiosk started, loading url");
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                println!("exit");
                *control_flow = ControlFlow::Exit
            }
            _ => (),
        }
    });
}
