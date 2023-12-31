use clap::Parser;
use std::path::PathBuf;
use std::{fs, path::Path, env};
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

        // Declare command_to_run and command_args outside of the match block
        let command_to_run = match service {
            "espflash" => "espflash",
            "qemu" => "qemu-system-riscv32",
            "wokwi" => "wokwi-cli",
            _ => return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Unknown service",
            ))),
        };

        let mut command_args = Vec::new();
        match service {
            "espflash" => {
                command_args.push("flash".to_string());
                command_args.push("-p".to_string());
                command_args.push("/dev/tty.usbmodem1101".to_string());
                command_args.push("--monitor".to_string());
                command_args.push(elf_path);
            },
            "qemu" => {
                if self.build_mode == "release" {
                    command_args.push("--release".to_string());
                }
                // Additional arguments for qemu
            },
            "wokwi" => {
                command_args.push("--elf".to_string());
                command_args.push(elf_path);
                command_args.push("--scenario".to_string());
                // Handle Option returned by to_str()
                let scenario_path_str = scenario_file.to_str().ok_or("Failed to convert scenario path to string")?.to_string();
                command_args.push(scenario_path_str);
                command_args.push("--timeout".to_string());
                command_args.push("5000".to_string());
                command_args.push("--serial-log-file".to_string());
                // Handle Option returned by to_str()
                let serial_log_file_str = serial_log_file.to_str().ok_or("Failed to convert path to string")?.to_string();
                command_args.push(serial_log_file_str);
            },
            _ => {} // Already handled above
        };

        let test_timeout = match service {
            "wokwi" => { Duration::from_secs(60) }
            _ => { Duration::from_secs(5) }
        };

        // Print command with arguments
        println!("Running: {} {:?}", command_to_run, command_args);

        let test_passed = match timeout(test_timeout, async {
            let mut command = TokioCommand::new(command_to_run);
            command.args(&command_args).current_dir(project_path)
                   .stdout(std::process::Stdio::piped())
                   .stderr(std::process::Stdio::piped());

            let output = command.output().await;
            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);

                    // Print stdout
                    for line in stdout.split('\n') {
                        println!("{}", line);
                    }

                    // Print stderr
                    for line in stderr.split('\n') {
                        eprintln!("{}", line);
                    }

                    output.status.success()
                }
                Err(e) => {
                    eprintln!("Command execution error: {}", e);
                    false
                }
            }
        }).await {
            Ok(result) => result,
            Err(_) => {
                eprintln!("Test {} timed out", example_name);
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
