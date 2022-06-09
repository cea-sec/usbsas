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
        process::{Child, Command, Stdio},
        thread::sleep,
        time::Duration,
    },
    usbsas_server::appstate,
};

struct IntegrationTester {
    api: String,
    client: Client,
    mock_input_dev: String,
    mock_output_dev: String,
    usbsas_server: Child,
    analyzer_server: Child,
}

impl IntegrationTester {
    fn new() -> Self {
        let test_data_dir = env::var("CARGO_MANIFEST_DIR")
            .expect("no CARGO_MANIFEST_DIR env var")
            .to_string()
            + "/test_data/";

        // Untar mock input dev if none was supplied
        let mock_input_dev = match env::var("USBSAS_MOCK_INPUT_DEV") {
            Ok(input) => {
                println!("Using {} as input dev", input);
                input
            }
            Err(_) => {
                let input = "/tmp/mock_input_dev.img";
                let input_file = std::fs::File::create(input).unwrap();
                Command::new("gzip")
                    .arg("-dc")
                    .arg(format!("{}/mock_input_dev.img.gz", test_data_dir))
                    .stdout(Stdio::from(input_file))
                    .stderr(Stdio::null())
                    .status()
                    .expect("Couldn't uncompress mock input dev");
                env::set_var("USBSAS_MOCK_IN_DEV", input);
                String::from(input)
            }
        };

        // Create mock output dev in none was supplied
        let mock_output_dev = match env::var("USBSAS_MOCK_OUTPUT_DEV") {
            Ok(output) => {
                println!("Using {} as output dev", output);
                output
            }
            Err(_) => {
                let output = "/tmp/mock_output_dev.img";
                Command::new("dd")
                    .arg("if=/dev/zero")
                    .arg("of=".to_string() + output)
                    .arg("bs=1M")
                    .arg("iflag=fullblock")
                    .arg("count=128")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("Couldn't create mock output dev");
                env::set_var("USBSAS_MOCK_OUT_DEV", output);
                String::from(output)
            }
        };

        // Start usbsas server
        let usbsas_server = Command::cargo_bin("usbsas-server")
            .expect("Couldn't run usbsas server")
            .args(&["-c", &format!("{}/config_test.toml", test_data_dir)])
            .spawn()
            .expect("Couldn't run usbsas server");

        // Start analyzer server
        let analyzer_server = Command::cargo_bin("usbsas-analyzer-server")
            .expect("Couldn't run analyzer server")
            .spawn()
            .expect("Couldn't run analyzer server");

        let client = Client::new();

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
            client: client,
            mock_input_dev,
            mock_output_dev,
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
        match self.client.get(&format!("{}{}", self.api, "reset")).send() {
            Ok(resp) => {
                assert!(resp.status().is_success());
            }
            Err(err) => {
                panic!("Couldn't reset: {}", err);
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
                self.list_files_recursive(files, &path)?;
            }
        }
        files.extend(new_files);
        Ok(())
    }

    fn do_copy(
        &self,
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

        // Find input dev (first USB)
        let input_dev = devices.swap_remove(
            devices
                .iter()
                .position(|dev| dev.dev_type == appstate::DevType::Usb)
                .expect("Couldn't find input usb dev"),
        );
        // Find output dev
        let output_dev = devices.swap_remove(
            devices
                .iter()
                .position(|dev| dev.dev_type == *output_type)
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

        // Get partitions
        let partitions: Vec<appstate::Partition> = self
            .client
            .get(&format!("{}{}", self.api, "devices/dirty"))
            .send()?
            .json()?;
        assert!(partitions.len() >= 1);

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
        // Filter directories of ok_path to ensure usbsas will still create them if they have children selected.
        let selected = files
            .iter()
            .filter(|(_, v)| v.ftype != 2 || !ok_path.contains(&(&*v.path_display)))
            .collect::<HashMap<&String, &appstate::ReadDir>>();
        // Select all files
        let post_payload = serde_json::json!({"selected": selected.into_keys().collect::<Vec<&String>>(),
            "fsfmt": fsfmt});

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
            .post(&format!("{}{}", self.api, "copy"))
            .json(&post_payload)
            .send()?;

        Ok(resp.text()?)
    }

    fn transfer(
        &self,
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
            if line == "" {
                continue;
            }
            let status: StatusJson = serde_json::from_str(&line).expect("quiche");
            if status.status == "final_report".to_string() {
                let report: appstate::ReportCopy = serde_json::from_str(&line).expect("plop");
                assert_eq!(report.dirty_path, dirty_path, "dirty path mismatch");
                assert_eq!(report.error_path, error_path, "error_path mismatch");
                assert_eq!(
                    report.filtered_path, filtered_path,
                    "filtered_path mismatch"
                );
            }
        }

        if let appstate::DevType::Usb = output_type {
            let sha1sum_cmd = Command::new("sha1sum")
                .args(&[self.mock_output_dev.clone()])
                .output()
                .expect("failed to execute sha1sum");
            let sha1sum = String::from_utf8(sha1sum_cmd.stdout).unwrap();
            assert_eq!(
                sha1sum.split_whitespace().nth(0).unwrap().to_string(),
                expected_sha1sum
            );
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

        // Find intput dev (first USB)
        let input_dev = devices
            .iter()
            .find(|dev| dev.dev_type == appstate::DevType::Usb)
            .unwrap();

        // Wipe dev
        let resp = self
            .client
            .get(&format!(
                "{}{}/{}/{}/{}",
                self.api,
                "wipe",
                input_dev.id,
                fsfmt,
                if quick { "true" } else { "false" }
            ))
            .send()?;
        assert!(resp.status().is_success());

        for line in resp.text()?.split("\r\n") {
            if line == "" {
                continue;
            }
            let status: StatusJson = serde_json::from_str(&line).unwrap();
            if status.status == "wipe_end".to_string() {
                let sha1sum_cmd = Command::new("sha1sum")
                    .args(&[self.mock_input_dev.clone()])
                    .output()
                    .expect("failed to execute sha1sum");
                let sha1sum = String::from_utf8(sha1sum_cmd.stdout).unwrap();
                assert_eq!(
                    sha1sum.split_whitespace().nth(0).unwrap().to_string(),
                    expected_sha1sum
                );
                return Ok(());
            }
        }
        Err(io::Error::new(io::ErrorKind::Other, "test failed").into())
    }

    fn dev_too_small(
        &self,
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
            if line == "" {
                continue;
            }
            let status: StatusJson = serde_json::from_str(&line).unwrap();
            if status.status == "copy_not_enough_space".to_string() {
                return;
            }
            if status.status == "final_report".to_string() {
                panic!("test transfer shouldn't finish");
            }
        }
    }
}

impl Drop for IntegrationTester {
    fn drop(&mut self) {
        // Send SIGTERM instead of SIGKILL so the signal is forwarded to sons
        loop {
            match signal::kill(Pid::from_raw(self.usbsas_server.id() as i32), SIGTERM) {
                Err(err) => {
                    println!("Couldn't sigterm usbsas server: {}", err);
                    sleep(Duration::from_secs(1));
                }
                Ok(_) => break,
            }
        }
        match self.analyzer_server.kill() {
            Err(e) => println!("Couldn't kill analyzer server: {}", e),
            Ok(_) => (),
        }
        // Remove mock {in,out}put dev
        let _ = fs::remove_file(&self.mock_input_dev).ok();
        let _ = fs::remove_file(&self.mock_output_dev).ok();
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
        "FAT",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
        "a9353e77e6a409a4143ff2d1fa26bb37c03b5872",
        "exfat",
    );
    tester.reset();

    // Test usb transfer NTFS -> FAT
    tester.transfer(
        appstate::DevType::Usb,
        "NTFS",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
        "daae66b7ddcae5a7873d415d1f4d3b17fcdfb621",
        "fat32",
    );
    tester.reset();

    // Test usb transfer ext4 -> NTFS
    tester.transfer(
        appstate::DevType::Usb,
        "Linux/Ext",
        &dirty_path,
        &error_path,
        &filtered_path,
        &ok_path,
        "75208a5631f31fae93d028dd7e96004c5e573c5c",
        "ntfs",
    );
    tester.reset();

    // Test upload
    tester.transfer(
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

    // Test transfer when output device is too small
    tester.dev_too_small(
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
        .wipe("fat32", true, "a38a4728650cce9a8314aaa322f8c8dd576d3e44")
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
