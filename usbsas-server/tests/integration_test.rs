use {
    assert_cmd::prelude::*,
    nix::{
        sys::signal::{self, SIGTERM},
        unistd::Pid,
    },
    reqwest::blocking::Client,
    serde::Deserialize,
    std::{
        collections::HashMap,
        env, fs, io,
        path::Path,
        process::{Child, Command, Stdio},
        thread::sleep,
        time::Duration,
    },
    usbsas_server::appstate,
};

struct IntegrationTester {
    api: String,
    client: Client,
    _mock_input_dev: String,
    mock_output_dev: String,
    working_dir: String,
    usbsas_server: Child,
    analyzer_server: Child,
}

impl IntegrationTester {
    fn new() -> Self {
        let test_data_dir =
            env::var("CARGO_MANIFEST_DIR").expect("no CARGO_MANIFEST_DIR env var") + "/test_data/";

        let working_dir = String::from("/tmp/usbsas-tests");

        if let Err(err) = fs::create_dir(&working_dir) {
            if err.kind() != io::ErrorKind::AlreadyExists {
                panic!("couldn't create working dir: {err}")
            }
        }

        // Untar mock input dev if none was supplied
        let mock_input_dev = match env::var("USBSAS_MOCK_INPUT_DEV") {
            Ok(input) => {
                println!("Using {input} as input dev");
                input
            }
            Err(_) => {
                let input = format!("{working_dir}/mock_input_dev.img");
                let input_file = std::fs::File::create(&input).unwrap();
                Command::new("gzip")
                    .arg("-dc")
                    .arg(format!("{test_data_dir}/mock_input_dev.img.gz"))
                    .stdout(Stdio::from(input_file))
                    .stderr(Stdio::null())
                    .status()
                    .expect("Couldn't uncompress mock input dev");
                env::set_var("USBSAS_MOCK_IN_DEV", &input);
                input
            }
        };

        // Create mock output dev if none was supplied
        let mock_output_dev = match env::var("USBSAS_MOCK_OUTPUT_DEV") {
            Ok(output) => {
                println!("Using {output} as output dev");
                output
            }
            Err(_) => {
                let output = format!("{working_dir}/mock_output_dev.img");
                Command::new("dd")
                    .arg("if=/dev/zero")
                    .arg(format!("of={}", &output))
                    .arg("bs=1M")
                    .arg("iflag=fullblock")
                    .arg("count=128")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("Couldn't create mock output dev");
                env::set_var("USBSAS_MOCK_OUT_DEV", &output);
                output
            }
        };

        // Copy export bundle in working_dir
        let _ = fs::create_dir(format!("{working_dir}/Tartempion"));
        Command::new("cp")
            .arg(format!("{test_data_dir}/bundle_test.tar.gz"))
            .arg(format!("{working_dir}/Tartempion/123456.tar.gz"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("Couldn't copy bundle test");

        // Start usbsas server
        let usbsas_server = Command::cargo_bin("usbsas-server")
            .expect("Couldn't run usbsas server")
            .args(["-c", &format!("{test_data_dir}/config_test.toml")])
            .spawn()
            .expect("Couldn't run usbsas server");

        // Start analyzer server
        let analyzer_server = Command::cargo_bin("usbsas-analyzer-server")
            .expect("Couldn't run analyzer server")
            .args(["-d", &working_dir])
            .spawn()
            .expect("Couldn't run analyzer server");

        let client = Client::builder()
            .timeout(None)
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("couldn't build reqwest client");

        // Wait for analyzer server to be ready (clamav db can be slow to load)
        loop {
            match client.get("http://127.0.0.1:8042/").send() {
                Ok(_) => break,
                Err(_) => {
                    sleep(Duration::from_secs(2));
                }
            }
        }

        IntegrationTester {
            api: "http://localhost:8080/".into(),
            client,
            _mock_input_dev: mock_input_dev,
            mock_output_dev,
            working_dir,
            usbsas_server,
            analyzer_server,
        }
    }

    fn reset(&self) {
        // Reset output dev
        Command::new("dd")
            .arg("if=/dev/zero")
            .arg(format!("of={}", self.mock_output_dev))
            .arg("bs=1M")
            .arg("iflag=fullblock")
            .arg("count=128")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("Couldn't reset mock output dev");
        match self.client.get(format!("{}{}", self.api, "reset")).send() {
            Ok(resp) => {
                assert!(resp.status().is_success());
            }
            Err(err) => {
                panic!("Couldn't reset: {err}");
            }
        }
    }

    fn list_files_recursive(
        &self,
        files: &mut HashMap<String, appstate::ReadDir>,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let new_files: HashMap<String, appstate::ReadDir> = self
            .client
            .get(&format!(
                "{}devices/dirty/read_dir/?path={}",
                self.api, path
            ))
            .send()?
            .json()?;
        for (path, file) in new_files.iter() {
            if file.ftype == 2 {
                self.list_files_recursive(files, path)?;
            }
        }
        files.extend(new_files);
        Ok(())
    }

    fn do_copy(
        &self,
        input_type: &appstate::DevType,
        output_type: &appstate::DevType,
        part_type: &str,
        dirty_path: &[&str],
        error_path: &[&str],
        filtered_path: &[&str],
        ok_path: &[&str],
        fsfmt: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Get Devices
        let mut devices: Vec<appstate::DeviceDesc> = self
            .client
            .get(&format!("{}{}", self.api, "devices"))
            .send()?
            .json()?;

        // Find input dev
        let input_dev = devices.swap_remove(
            devices
                .iter()
                .position(|dev| dev.dev_type == *input_type && dev.is_src)
                .expect("Couldn't find input dev"),
        );
        // Find output dev
        let output_dev = devices.swap_remove(
            devices
                .iter()
                .position(|dev| dev.dev_type == *output_type && dev.is_dst)
                .expect("Couldn't find output dev"),
        );

        // Select devices
        let resp = self
            .client
            .get(&format!(
                "{}{}/{}/{}",
                self.api, "devices/select", input_dev.id, output_dev.id
            ))
            .send()?;
        assert!(resp.status().is_success());

        let post_payload = match *input_type {
            appstate::DevType::Usb => {
                // Get partitions
                let partitions: Vec<appstate::Partition> = self
                    .client
                    .get(&format!("{}{}", self.api, "devices/dirty"))
                    .send()?
                    .json()?;
                assert!(!partitions.is_empty());

                // Find partition
                let part_index = partitions
                    .iter()
                    .find(|&part| part.type_str == part_type)
                    .unwrap()
                    .index;

                // Open partition
                let resp = self
                    .client
                    .get(&format!(
                        "{}{}{}",
                        self.api, "devices/dirty/open/", part_index
                    ))
                    .send()?;
                assert!(resp.status().is_success());

                // Read / directory
                let mut files: HashMap<String, appstate::ReadDir> = HashMap::new();

                // "" for listing root dir
                self.list_files_recursive(&mut files, "")?;

                let all_path: Vec<&&str> = ok_path
                    .iter()
                    .chain(error_path.iter())
                    .chain(dirty_path.iter())
                    .chain(filtered_path.iter())
                    .collect();
                files
                    .values()
                    .for_each(|file| assert!(all_path.contains(&&file.path_display.as_str())));
                // Filter directories of ok_path to ensure usbsas will still
                // create them if they have children selected.
                let selected = files
                    .iter()
                    .filter(|(_, v)| v.ftype != 2 || !ok_path.contains(&(&*v.path_display)))
                    .collect::<HashMap<&String, &appstate::ReadDir>>();
                // Select all files
                serde_json::json!({"selected": selected.into_keys().collect::<Vec<&String>>(), "fsfmt": fsfmt})
            }
            appstate::DevType::Net => {
                serde_json::json!({"selected": Vec::<String>::new(), "fsfmt": fsfmt, "download_pin": "123456"})
            }
            _ => panic!("input_dev shouldn't be this"),
        };

        // Get id before copy
        let id = loop {
            let resp = self
                .client
                .get(&format!("{}{}", self.api, "id"))
                .send()?
                .text()?;
            sleep(Duration::from_secs(1));
            if resp != "\"\"" {
                break resp;
            }
        };
        assert_eq!(id, "\"Tartempion\"");

        // Start copy
        let resp = self
            .client
            .post(format!("{}{}", self.api, "copy"))
            .json(&post_payload)
            .send()?;

        Ok(resp.text()?)
    }

    fn transfer(
        &self,
        input_type: appstate::DevType,
        output_type: appstate::DevType,
        part_type: &str,
        dirty_path: &[&str],
        error_path: &[&str],
        filtered_path: &[&str],
        ok_path: &[&str],
        expected_sha1sum: &str,
        fsfmt: &str,
    ) {
        let resp = self
            .do_copy(
                &input_type,
                &output_type,
                part_type,
                dirty_path,
                error_path,
                filtered_path,
                ok_path,
                fsfmt,
            )
            .expect("copy failed");

        for line in resp.split("\r\n") {
            if line.is_empty() {
                continue;
            }
            let status: StatusJson = serde_json::from_str(line).expect("quiche");
            if status.status == *"final_report" {
                let response: appstate::ReportCopy = serde_json::from_str(line).expect("plop");
                assert!(response.report["file_names"].as_array().is_some());
                if let Some(resp_filtered) = response.report["filtered_files"].as_array() {
                    assert_eq!(resp_filtered, filtered_path, "filtered_path mismatch");
                }

                if let Some(resp_error) = response.report["error_files"].as_array() {
                    assert_eq!(resp_error, error_path, "error_path mismatch");
                }

                if let Some(analyzed_files) = response.report["analyzer_report"].as_object() {
                    let mut resp_dirty: Vec<String> = Vec::new();
                    for (file, status) in analyzed_files["files"].as_object().unwrap() {
                        if status["status"] == "DIRTY" {
                            resp_dirty.push(format!("/{}", file));
                        }
                    }
                    assert_eq!(resp_dirty, dirty_path, "dirty path mismatch");
                }
            }
        }

        if let appstate::DevType::Usb = output_type {
            let sha1sum_cmd = Command::new("sha1sum")
                .args(&[self.mock_output_dev.clone()])
                .output()
                .expect("failed to execute sha1sum");
            let sha1sum = String::from_utf8(sha1sum_cmd.stdout).unwrap();
            assert_eq!(
                sha1sum.split_whitespace().next().unwrap().to_string(),
                expected_sha1sum
            );
        } else if let appstate::DevType::Net = output_type {
            assert!(Path::new(&format!("{}/bundle_test.tar", self.working_dir)).exists());
        }
    }

    fn wipe(
        &self,
        fsfmt: &str,
        quick: bool,
        expected_sha1sum: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Get devices
        let devices: Vec<appstate::DeviceDesc> = self
            .client
            .get(&format!("{}{}", self.api, "devices"))
            .send()?
            .json()?;

        // Find input dev (first USB)
        let device = devices
            .iter()
            .find(|dev| dev.dev_type == appstate::DevType::Usb && dev.is_dst)
            .unwrap();

        // Wipe dev
        let resp = self
            .client
            .get(&format!(
                "{}{}/{}/{}/{}",
                self.api,
                "wipe",
                device.id,
                fsfmt,
                if quick { "true" } else { "false" }
            ))
            .send()?;
        assert!(resp.status().is_success());

        for line in resp.text()?.split("\r\n") {
            if line.is_empty() {
                continue;
            }
            let status: StatusJson = serde_json::from_str(line).unwrap();
            if status.status == *"wipe_end" {
                let sha1sum_cmd = Command::new("sha1sum")
                    .args(&[self.mock_output_dev.clone()])
                    .output()
                    .expect("failed to execute sha1sum");
                let sha1sum = String::from_utf8(sha1sum_cmd.stdout).unwrap();
                assert_eq!(
                    sha1sum.split_whitespace().next().unwrap().to_string(),
                    expected_sha1sum
                );
                return Ok(());
            }
        }
        Err(io::Error::new(io::ErrorKind::Other, "test failed").into())
    }

    fn dev_too_small(
        &self,
        input_type: appstate::DevType,
        output_type: appstate::DevType,
        part_type: &str,
        dirty_path: &[&str],
        error_path: &[&str],
        filtered_path: &[&str],
        ok_path: &[&str],
    ) {
        // Create 1MB output device
        Command::new("dd")
            .arg("if=/dev/zero")
            .arg("of=".to_string() + &self.mock_output_dev)
            .arg("bs=1M")
            .arg("iflag=fullblock")
            .arg("count=1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("Couldn't create mock output dev");

        let resp = self
            .do_copy(
                &input_type,
                &output_type,
                part_type,
                dirty_path,
                error_path,
                filtered_path,
                ok_path,
                "fat32",
            )
            .expect("copy failed");

        for line in resp.split("\r\n") {
            if line.is_empty() {
                continue;
            }
            let status: StatusJson = serde_json::from_str(line).unwrap();
            if status.status == *"copy_not_enough_space" {
                return;
            }
            if status.status == *"final_report" {
                panic!("test transfer shouldn't finish");
            }
        }
    }
}

impl Drop for IntegrationTester {
    fn drop(&mut self) {
        // Send SIGTERM instead of SIGKILL so the signal is forwarded to sons
        while let Err(err) = signal::kill(Pid::from_raw(self.usbsas_server.id() as i32), SIGTERM) {
            println!("Couldn't sigterm usbsas server: {err}");
            sleep(Duration::from_secs(1));
        }
        if let Err(e) = self.analyzer_server.kill() {
            println!("Couldn't kill analyzer server: {e}");
        }
        sleep(Duration::from_secs(1));
        // Remove working dir
        let _ = fs::remove_dir_all(&self.working_dir).ok();
    }
}

#[derive(Debug, Deserialize)]
struct StatusJson {
    status: String,
}

#[test]
fn integration_test() {
    let tester = IntegrationTester::new();
    tester.reset();

    // Files in all 3 partitions of test_data/mock_input_dev.img
    let dirty_path = ["/eicar.com"];
    let error_path = [];
    let filtered_path = ["/AUTORUN.INF", "/Micro$oft.lnk", "/.DS_STORE"];
    let ok_path = [
        "/directories",
        "/directories/a",
        "/directories/a/man_rustc.txt",
        "/directories/b",
        "/directories/b/c",
        "/directories/b/c/man_cargo.txt",
        "/tree 🌲.txt",
        "/SCSI Commands Reference Manual.pdf",
    ];

    // Test usb transfer FAT -> ExFAT
    tester.transfer(
        appstate::DevType::Usb,
        appstate::DevType::Usb,
        "FAT",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
        "1f379595281e600cb36bcb1965768908ba889cb8",
        "exfat",
    );
    tester.reset();

    // Test usb transfer NTFS -> FAT
    tester.transfer(
        appstate::DevType::Usb,
        appstate::DevType::Usb,
        "NTFS",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
        "1981ab7ba38967038444ec1d56ab1a4f2616802c",
        "fat32",
    );
    tester.reset();

    // Test usb transfer ext4 -> NTFS
    tester.transfer(
        appstate::DevType::Usb,
        appstate::DevType::Usb,
        "Linux/Ext",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
        "55f5736545687274240e8b4ca84247e3dcf42415",
        "ntfs",
    );
    tester.reset();

    // Test upload
    tester.transfer(
        appstate::DevType::Usb,
        appstate::DevType::Net,
        "FAT",
        &[],
        &[],
        &filtered_path,
        &[&ok_path[..], &dirty_path[..], &error_path[..]].concat(),
        "",
        "",
    );
    tester.reset();

    // Test download
    tester.transfer(
        appstate::DevType::Net,
        appstate::DevType::Usb,
        "",
        &[],
        &[],
        &[],
        &[&ok_path[..], &[], &[]].concat(),
        "903dc5219a4de94a75449ee607da49edc5b78e77",
        "ntfs",
    );
    tester.reset();

    // Test transfer when output device is too small
    tester.dev_too_small(
        appstate::DevType::Usb,
        appstate::DevType::Usb,
        "FAT",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
    );
    tester.reset();

    // Test quick wipe & mkfs fat32
    tester
        .wipe("fat32", true, "76fec4a87ce5a5e0157afc91fd603b272402629f")
        .expect("wipe failed");
    tester.reset();

    // Test secure wipe & mkfs ntfs
    tester
        .wipe("ntfs", false, "94c03f01de25aa834a2c1572de1efb672e6ebdb8")
        .expect("wipe failed");
    tester.reset();

    drop(tester);
    // Time to stop properly
    sleep(Duration::from_secs(2));
}
