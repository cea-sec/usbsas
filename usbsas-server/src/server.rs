use crate::appstate::{AppState, CopyIn, DeviceDesc, ReadDirQuery, ResponseStream, UsbsasInfos};
use crate::error::ServiceError;
use crate::srv_infos::get_server_infos;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use log::info;
use std::{
    collections::HashMap,
    io::{self, ErrorKind},
    thread,
};
use usbsas_config::{conf_parse, conf_read};

#[get("/id")]
async fn id(data: web::Data<AppState>) -> Result<impl Responder, ServiceError> {
    let id = data.id()?;
    Ok(HttpResponse::Ok().json(id))
}

#[get("/usbsas_infos")]
async fn usbsas_infos(data: web::Data<AppState>) -> Result<impl Responder, ServiceError> {
    let node_name = match uname::Info::new() {
        Ok(infos) => infos.nodename,
        _ => "Unknown".to_string(),
    };
    /* Re-read conf to get message updates */
    let config_str = conf_read(&data.config_path.lock()?)?;
    let config = conf_parse(&config_str)?;

    Ok(HttpResponse::Ok().json(UsbsasInfos {
        name: node_name,
        message: config.message.unwrap_or_else(|| "".into()),
        version: usbsas_utils::USBSAS_VERSION.into(),
    }))
}

#[get("/server_infos")]
async fn server_infos() -> Result<impl Responder, ServiceError> {
    Ok(HttpResponse::Ok().json(get_server_infos()))
}

#[get("/devices")]
async fn devices(data: web::Data<AppState>) -> Result<impl Responder, ServiceError> {
    let devices = data.list_all_devices()?;
    let devices: Vec<DeviceDesc> = devices.iter().map(DeviceDesc::from).collect();
    Ok(HttpResponse::Ok().json(devices))
}

#[get("/devices/select/{fingerprint_dirty}/{fingerprint_out}")]
async fn device_select(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder, ServiceError> {
    let (fingerprint_dirty, fingerprint_out) = params.into_inner();
    data.device_select(fingerprint_dirty, fingerprint_out)?;
    Ok(HttpResponse::Ok())
}

#[get("/devices/dirty")]
async fn read_partitions(data: web::Data<AppState>) -> Result<impl Responder, ServiceError> {
    let partitions = data.read_partitions()?;
    Ok(HttpResponse::Ok().json(partitions))
}

#[get("/devices/dirty/open/{num}")]
async fn open_partition(
    params: web::Path<u32>,
    data: web::Data<AppState>,
) -> Result<impl Responder, ServiceError> {
    let num = params.into_inner();
    data.open_partition(num)?;
    Ok(HttpResponse::Ok())
}

#[get("/devices/dirty/read_dir/")]
async fn read_dir(
    query: web::Query<ReadDirQuery>,
    data: web::Data<AppState>,
) -> Result<impl Responder, ServiceError> {
    let ret = data.read_dir(&query.path)?;
    // Create a dict of it, avoiding a copy of ReadDir.path for key
    let dict: HashMap<_, _> = ret.iter().map(|rdj| (&rdj.path, rdj)).collect();
    Ok(HttpResponse::Ok().json(dict))
}

#[post("/copy")]
async fn copy(
    files: web::Json<CopyIn>,
    data: web::Data<AppState>,
) -> Result<impl Responder, ServiceError> {
    let resp_stream = ResponseStream::new();
    let resp_stream_clone = resp_stream.clone();
    thread::spawn(move || {
        let _ = data.copy(
            files.selected.to_owned(),
            files.fsfmt.to_owned(),
            resp_stream_clone,
        );
    });
    Ok(HttpResponse::Ok().streaming(resp_stream))
}

#[get("/wipe/{fingertprint}/{fsfmt}/{quick}")]
async fn wipe(
    params: web::Path<(String, String, bool)>,
    data: web::Data<AppState>,
) -> Result<impl Responder, ServiceError> {
    let (fingerprint, fsfmt, quick) = params.into_inner();
    let device = data.dev_from_fingerprint(fingerprint)?;
    let resp_stream = ResponseStream::new();
    let resp_stream_clone = resp_stream.clone();
    thread::spawn(move || {
        let _ = data.wipe(device, fsfmt, quick, resp_stream_clone);
    });
    Ok(HttpResponse::Ok().streaming(resp_stream))
}

#[get("/imagedisk/{fingerprint}")]
async fn imagedisk(
    params: web::Path<String>,
    data: web::Data<AppState>,
) -> Result<impl Responder, ServiceError> {
    let fingerprint = params.into_inner();
    let device = data.dev_from_fingerprint(fingerprint)?;
    let resp_stream = ResponseStream::new();
    let resp_stream_clone = resp_stream.clone();
    thread::spawn(move || {
        let _ = data.imagedisk(device, resp_stream_clone);
    });
    Ok(HttpResponse::Ok().streaming(resp_stream))
}

#[get("/reset")]
async fn reset(data: web::Data<AppState>) -> Result<impl Responder, ServiceError> {
    info!("** Resetting server **");
    data.reset()?;
    Ok(HttpResponse::Ok())
}

#[get("/")]
async fn index() -> Result<impl Responder, ServiceError> {
    Ok(actix_files::NamedFile::open(format!(
        "{}/index.html",
        env!("USBSAS_WEBFILES_DIR")
    ))?)
}

#[actix_web::main]
pub async fn start_server(config_path: String) -> io::Result<()> {
    let app_data = web::Data::new(AppState::new(config_path).map_err(|err| {
        io::Error::new(
            ErrorKind::Other,
            format!("couldn't init server data: {}", err),
        )
    })?);
    #[cfg(feature = "log-json")]
    usbsas_utils::log::init_logger(app_data.session_id.clone());
    #[cfg(not(feature = "log-json"))]
    usbsas_utils::log::init_logger();
    HttpServer::new(move || {
        App::new()
            .app_data(app_data.clone())
            .wrap(
                // Polled regularly by client, too noisy
                actix_web::middleware::Logger::default()
                    .exclude("/id")
                    .exclude("/devices")
                    .exclude("/status")
                    .exclude("/static"),
            )
            .service(id)
            .service(usbsas_infos)
            .service(server_infos)
            .service(devices)
            .service(device_select)
            .service(read_partitions)
            .service(open_partition)
            .service(read_dir)
            .service(copy)
            .service(wipe)
            .service(imagedisk)
            .service(reset)
            .service(actix_files::Files::new(
                "/static/",
                format!("{}/static/", env!("USBSAS_WEBFILES_DIR")),
            ))
            .service(index)
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
