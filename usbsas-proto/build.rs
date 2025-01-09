fn main() {
    let proto_files = [
        "proto/analyzer.proto3",
        "proto/identificator.proto3",
        "proto/cmdexec.proto3",
        "proto/common.proto3",
        "proto/downloader.proto3",
        "proto/files.proto3",
        "proto/fs2dev.proto3",
        "proto/scsi.proto3",
        "proto/uploader.proto3",
        "proto/usbdev.proto3",
        "proto/usbsas.proto3",
        "proto/writedst.proto3",
    ];

    // Compile & gen protobuf code
    let mut prost_build = prost_build::Config::new();
    prost_build.message_attribute("common.Network", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.UsbDevice", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.Command", "#[derive(Hash, serde::Serialize)]");
    prost_build.enum_attribute("common.Device.device", "#[derive(Hash, serde::Serialize)]");
    prost_build
        .compile_protos(&proto_files, &["proto"])
        .expect("Couldn't build protobuf files");

    proto_files
        .iter()
        .for_each(|x| println!("cargo:rerun-if-changed={x}"));
}
