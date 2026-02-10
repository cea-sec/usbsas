fn main() {
    let proto_files = [
        "proto/analyzer.proto3",
        "proto/identifier.proto3",
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
        "common.TransferReport.files_info",
    ]);

    // Impl serde::Serialize / Deserialize
    prost_build.message_attribute("common.Network", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.UsbDevice", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.Command", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.LocalDir", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.TransferReport", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.DeviceReport", "#[derive(serde::Serialize)]");
    prost_build.message_attribute(
        "common.AnalyzeReport",
        "#[derive(serde::Serialize, serde::Deserialize)]",
    );
    prost_build.message_attribute("common.NetworkReport", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.CommandReport", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.LocalDirReport", "#[derive(serde::Serialize)]");
    prost_build.enum_attribute("common.DeviceReport.device", "#[derive(serde::Serialize)]");
    prost_build.message_attribute("common.PartitionInfo", "#[derive(serde::Serialize)]");
    prost_build.enum_attribute("common.Device.device", "#[derive(serde::Serialize)]");

    prost_build.message_attribute(
        "common.FileStatus",
        "#[derive(serde::Serialize, serde::Deserialize)]",
    );
    prost_build.message_attribute(
        "common.Antivirus",
        "#[derive(serde::Serialize, serde::Deserialize)]",
    );
    prost_build.message_attribute("jsonparser.SrvResp", "#[derive(serde::Deserialize)]");
    prost_build.message_attribute("common.UsbDeviceReport", "#[derive(serde::Serialize)]");
    prost_build.message_attribute(
        "common.FileInfoReport",
        "#[derive(serde::Serialize, serde::Deserialize)]",
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
        "common.TransferReport.files",
        "#[serde(skip_serializing_if = \"BTreeMap::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.errors",
        "#[serde(skip_serializing_if = \"Vec::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.filtered",
        "#[serde(skip_serializing_if = \"Vec::is_empty\")]",
    );
    prost_build.field_attribute(
        "common.TransferReport.rejected",
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
    prost_build.field_attribute(
        "common.FileInfoReport.sha256",
        "#[serde(skip_serializing_if = \"Option::is_none\")]",
    );

    prost_build
        .compile_protos(&proto_files, &["proto"])
        .expect("Couldn't build protobuf files");

    proto_files
        .iter()
        .for_each(|x| println!("cargo:rerun-if-changed={x}"));
}
