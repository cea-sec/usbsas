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
        "proto/jsonparser.proto3",
    ];

    // Compile & gen protobuf code
    let mut prost_build = prost_build::Config::new();

    // BTreeMap because HashMap doesn't impl Hash
    prost_build.btree_map([
        "common.AnalyzeReport.antivirus",
        "common.AnalyzeReport.files",
    ]);

    // Impl serde::Serialize / Deserialize
    prost_build.message_attribute("common.Network", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.UsbDevice", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.Command", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.TransferReport", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.DeviceReport", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute(
        "common.AnalyzeReport",
        "#[derive(Hash, serde::Serialize, serde::Deserialize)]",
    );
    prost_build.message_attribute("common.NetworkReport", "#[derive(Hash, serde::Serialize)]");
    prost_build.message_attribute("common.CommandReport", "#[derive(Hash, serde::Serialize)]");
    prost_build.enum_attribute(
        "common.DeviceReport.device",
        "#[derive(Hash, serde::Serialize)]",
    );
    prost_build.message_attribute("common.PartitionInfo", "#[derive(Hash, serde::Serialize)]");
    prost_build.enum_attribute("common.Device.device", "#[derive(Hash, serde::Serialize)]");

    prost_build.message_attribute(
        "common.FileStatus",
        "#[derive(Hash, serde::Serialize, serde::Deserialize)]",
    );
    prost_build.message_attribute(
        "common.Antivirus",
        "#[derive(Hash, serde::Serialize, serde::Deserialize)]",
    );
    prost_build.message_attribute("jsonparser.SrvResp", "#[derive(Hash, serde::Deserialize)]");
    prost_build.message_attribute(
        "common.UsbDeviceReport",
        "#[derive(Hash, serde::Serialize)]",
    );

    // Skip serializing if None
    prost_build.field_attribute(
        "common.TransferReport.user",
        "#[serde(skip_serializing_if = \"Option::is_none\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.source",
        "#[serde(skip_serializing_if = \"Option::is_none\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.destination",
        "#[serde(skip_serializing_if = \"Option::is_none\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.file_names",
        "#[serde(skip_serializing_if = \"Vec::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.error_files",
        "#[serde(skip_serializing_if = \"Vec::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.filtered_files",
        "#[serde(skip_serializing_if = \"Vec::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.rejected_files",
        "#[serde(skip_serializing_if = \"Vec::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.analyzereport",
        "#[serde(skip_serializing_if = \"Option::is_none\")]",
    );
    prost_build.field_attribute(
        "common.FileStatus.sha256",
        "#[serde(skip_serializing_if = \"Option::is_none\")]",
    );

    prost_build
        .compile_protos(&proto_files, &["proto"])
        .expect("Couldn't build protobuf files");

    proto_files
        .iter()
        .for_each(|x| println!("cargo:rerun-if-changed={x}"));
}
