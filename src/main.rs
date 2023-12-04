use clap::Parser;
use std::path::PathBuf;
use std::{fs, path::Path};
use std::process::Stdio;
use tokio::time::{self, Duration, timeout};
use tokio::process::Command as TokioCommand;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    project_path: String,

    #[arg(short, long)]
    output_directory: String,

    #[arg(short, long)]
    continue_on_error: bool,

    #[arg(short = 'n', long)]
    skip_build: bool,

    #[arg(short = 'j', long, default_value_t = 0)]
    parallelism: usize,

    #[arg(short, long, default_value = "wokwi")]
    service: String,
}

#[derive(Clone)]
struct TestCase {
    file_path: String,
    build_mode: String,
}

impl TestCase {
    fn build(&self, project_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let example_name = Path::new(&self.file_path)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or("Failed to extract example name")?;

        println!("Building {} in {} mode...", example_name, self.build_mode);

        let mut command = std::process::Command::new("cargo");
        command.args(["build", "--example", example_name])
               .current_dir(project_path);
        if self.build_mode == "release" {
            command.arg("--release");
        }

        let status = command.status()?;
        if !status.success() {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to build {}", example_name),
            )));
        }

        Ok(())
    }


    async fn run(&self, project_path: &Path, output_directory: &Path, service: &str, continue_on_error: bool) -> Result<bool, Box<dyn std::error::Error>> {
        let example_name = Path::new(&self.file_path)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or("Failed to construct ELF path")?;

        let elf_path = format!(
            "target/riscv32imac-unknown-none-elf/{}/examples/{}",
            if self.build_mode == "debug" { "debug" } else { "release" },
            example_name
        );

        let scenario_dir = Path::new("scenarios").join(&self.build_mode);
        let scenario_file = scenario_dir.join(format!("{}.yaml", example_name));

        let absolute_output_dir = output_directory.canonicalize()?;
        let tmp_output_dir = absolute_output_dir.join("tmp");
        fs::create_dir_all(&tmp_output_dir)?;
        let serial_log_file = tmp_output_dir.join(format!("{}-{}.txt", example_name, self.build_mode));

        let mut command_args = Vec::new();
        let command_to_run = match service {
            "espflash" => {
                command_args.push("flash".to_string());
                command_args.push("-p".to_string());
                command_args.push("/dev/tty.usbmodem1101".to_string());
                command_args.push("--monitor".to_string());
                command_args.push(elf_path.clone());
                "espflash"
            },
            "qemu" => {
                if self.build_mode == "release" {
                    command_args.push("--release".to_string());
                }
                "qemu-system-riscv32"
            },
            "wokwi" => {
                command_args.push("--elf".to_string());
                command_args.push(elf_path.clone());
                command_args.push("--scenario".to_string());
                command_args.push(scenario_file.to_str().ok_or("Failed to convert scenario path to string")?.to_string());
                command_args.push("--timeout".to_string());
                command_args.push("5000".to_string());
                command_args.push("--serial-log-file".to_string());
                command_args.push(serial_log_file.to_str().ok_or("Failed to convert path to string")?.to_string());
                "wokwi-cli"
            },
            _ => {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Unknown service",
                )));
            }
        };

        let mut command = TokioCommand::new(command_to_run);
        command.args(&command_args)
               .current_dir(project_path)
               .stdout(Stdio::piped())
               .stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let child_id = child.id().expect("Failed to get child process id");

        let test_timeout = Duration::from_secs(5);
        let test_passed = match timeout(test_timeout, child.wait_with_output()).await {
            Ok(result) => match result {
                Ok(output) => {
                    if service == "espflash" {
                        fs::write(&serial_log_file, &output.stdout)?;
                    }
                    output.status.success()
                },
                Err(e) => {
                    eprintln!("Command execution error: {}", e);
                    false
                }
            },
            Err(_) => {
                eprintln!("Test {} timed out", example_name);
                kill_child_process(child_id).await?;
                false
            }
        };

        let result_dir = if test_passed { "passed" } else { "failed" };
        let final_output_dir = absolute_output_dir.join(result_dir);
        fs::create_dir_all(&final_output_dir)?;
        fs::rename(&serial_log_file, final_output_dir.join(format!("{}-{}.txt", example_name, self.build_mode)))?;

        if !test_passed && !continue_on_error {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Test failed and continue_on_error is false",
            )));
        }

        Ok(test_passed)
    }
}


fn discover_test_cases(path: &Path) -> Vec<TestCase> {
    let mut test_cases = Vec::new();

    let examples_path = path.join("examples");
    if examples_path.is_dir() {
        if let Ok(entries) = fs::read_dir(examples_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs") {
                    test_cases.push(TestCase {
                        file_path: path.to_string_lossy().into_owned(),
                        build_mode: "debug".to_string(),
                    });
                    test_cases.push(TestCase {
                        file_path: path.to_string_lossy().into_owned(),
                        build_mode: "release".to_string(),
                    });
                }
            }
        }
    }
    test_cases.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    test_cases
}

async fn kill_child_process(child_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    // Use a command line utility to kill the process
    let kill_command = std::process::Command::new("kill")
        .arg(format!("{}", child_id))
        .spawn()?
        .wait()
        .expect("Failed to kill the child process");

    if kill_command.success() {
        Ok(())
    } else {
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to kill the timed out process",
        )))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let project_path = PathBuf::from(&args.project_path);
    let output_directory = PathBuf::from(&args.output_directory);

    let service = args.service;
    let parallelism = if args.parallelism == 0 { num_cpus::get() } else { args.parallelism };

    let test_cases = discover_test_cases(&project_path);

    let mut passed_tests = 0;
    let mut failed_tests = 0;
    let mut total_build_time = Duration::new(0, 0);
    let mut total_test_time = Duration::new(0, 0);

    if !args.skip_build {
        for test in &test_cases {
            let start = time::Instant::now();
            test.build(&project_path)?;
            total_build_time += start.elapsed();
        }
    }

    let test_start = time::Instant::now();

    let mut handles = Vec::new();
    for test in test_cases {
        let project_path_clone = project_path.clone();
        let output_directory_clone = output_directory.clone();
        let service_clone = service.clone();

        let handle = tokio::spawn(async move {
            test.run(&project_path_clone, &output_directory_clone, &service_clone, args.continue_on_error).await
                .map(|result| (result, test.file_path.clone(), test.build_mode.clone()))
                .unwrap_or_else(|e| {
                    println!("Error: {}", e);
                    (false, test.file_path.clone(), test.build_mode.clone())
                })
        });

        handles.push(handle);
        if handles.len() == parallelism {
            for handle in handles.drain(..) {
                let (result, file_path, build_mode) = handle.await.unwrap();
                if result {
                    passed_tests += 1;
                } else {
                    failed_tests += 1;
                }
                println!("Test {} in {} mode: {}", file_path, build_mode, if result { "passed" } else { "failed" });
            }
        }
    }

    for handle in handles {
        let (result, file_path, build_mode) = handle.await.unwrap();
        if result {
            passed_tests += 1;
        } else {
            failed_tests += 1;
        }
        println!("Test {} in {} mode: {}", file_path, build_mode, if result { "passed" } else { "failed" });
    }

    total_test_time += test_start.elapsed();

    println!("Test run summary:");
    println!("Total build time: {:?}", total_build_time);
    println!("Total test time: {:?}", total_test_time);
    println!("Passed tests: {}", passed_tests);
    println!("Failed tests: {}", failed_tests);

    Ok(())
}
