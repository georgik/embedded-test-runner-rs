use clap::Parser;
use tokio::process::Command as TokioCommand;
use serde::Deserialize;
use serde_yaml;
use std::process::Stdio;
use std::path::PathBuf;
use tokio::time::{timeout, Duration};
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    elf_path: String,

    #[arg(short, long)]
    service: String,

    #[arg(short, long)]
    timeout: Option<u64>,

    #[arg(short = 'p', long)]
    scenario: Option<PathBuf>,

    #[arg(short, long)]
    output_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    steps: Vec<ScenarioStep>,
}

#[derive(Debug, Deserialize)]
struct ScenarioStep {
    #[serde(rename = "wait-serial")]
    wait_serial: String,
}

async fn run_test(args: Args) -> Result<bool, Box<dyn std::error::Error>> {
    let elf_file_path = args.elf_path;
    let selected_service = args.service;
    let scenario_file_path = args.scenario;
    let test_output_file_path = args.output_file;

    let mut command_builder = match selected_service.as_str() {
        "espflash" => {
            let mut espflash_command = TokioCommand::new("espflash");
            espflash_command.arg("flash")
                .arg("-p")
                .arg("/dev/tty.usbmodem1101")
                .arg("--monitor")
                .arg(&elf_file_path);

            println!("Executing command: espflash flash --monitor {}", elf_file_path);
            espflash_command
        },
        "qemu" => {
            let mut qemu_command = TokioCommand::new("qemu-system-riscv32");
            qemu_command.arg(&elf_file_path);

            println!("Executing command: qemu-system-riscv32 {}", elf_file_path);
            qemu_command
        },
        "wokwi" => {
            let mut wokwi_command = TokioCommand::new("wokwi-cli");
            wokwi_command.arg("--elf").arg(&elf_file_path);

            let mut command_message = format!("Executing command: wokwi-cli --elf {}", elf_file_path);

            if let Some(scenario_path) = &scenario_file_path {
                wokwi_command.arg("--scenario").arg(scenario_path);
                command_message.push_str(&format!(" --scenario {}", scenario_path.display()));
            }

            if let Some(timeout) = args.timeout.map(Duration::from_secs) {
                wokwi_command.arg("--timeout").arg(timeout.as_secs().to_string());
                command_message.push_str(&format!(" --timeout {}", timeout.as_secs()));
            }

            if let Some(output_file_path) = &test_output_file_path {
                wokwi_command.arg("--serial-log-file").arg(output_file_path);
                command_message.push_str(&format!(" --serial-log-file {}", output_file_path.display()));
            }

            println!("{}", command_message);
            wokwi_command
        },
        _ => return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unknown service",
        ))),
    };

    command_builder.stdout(Stdio::piped()).stderr(Stdio::piped());

    let test_timeout_duration = args.timeout.map_or(Duration::from_secs(60), Duration::from_secs);

    let child = command_builder.spawn()?;
    let shared_child = Arc::new(Mutex::new(child));
    let shared_child_clone = Arc::clone(&shared_child); // Clone the Arc

    // Read and parse the scenario file, if provided
    let scenario = if let Some(scenario_path) = &scenario_file_path {
        let file_contents = tokio::fs::read_to_string(scenario_path).await?;
        match serde_yaml::from_str::<Scenario>(&file_contents) {
            Ok(scenario) => Some(scenario),
            Err(e) => {
                let error: Box<dyn std::error::Error> = Box::new(e);
                return Err(error);
            }
        }
    } else {
        println!("No scenario file provided");
        None
    };

    let child_future = async move {
        let mut child_guard = shared_child_clone.lock().await;
        let child = &mut *child_guard;

        let child_stdout = child.stdout.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stdout"))?;
        let mut reader = BufReader::new(child_stdout);
        let mut async_stdout = tokio::io::stdout();
        let mut child_stderr = child.stderr.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stderr"))?;
        let mut async_stderr = tokio::io::stderr();

        let scenario_match = if let Some(scenario) = &scenario {
            let mut scenario_success = false;
            let mut line = String::new();

            while reader.read_line(&mut line).await? != 0 {
                // Output the line to stdout
                async_stdout.write_all(line.as_bytes()).await?;

                // Check if the line matches any step in the scenario
                for step in scenario.steps.iter() {
                    if line.contains(&step.wait_serial) {
                        scenario_success = true;
                        break;
                    }
                }
                line.clear();

                // Break if scenario success
                if scenario_success {
                    child.kill().await?;  // Terminate espflash on scenario success
                    break;
                }
            }
            scenario_success
        } else {
            false  // No scenario provided
        };

        // Copy stderr to async stderr
        let stderr_fut = tokio::io::copy(&mut child_stderr, &mut async_stderr);
        let _ = stderr_fut.await;

        Ok::<bool, Box<dyn std::error::Error>>(scenario_match)
    };

    match timeout(test_timeout_duration, child_future).await {
        Ok(result) => {
            result.map_err(Into::into)
        },
        Err(_) => {
            eprintln!("Test execution timed out after {:?} seconds", test_timeout_duration.as_secs());
            let mut child_guard = shared_child.lock().await;
            child_guard.kill().await?;  // Terminate espflash on timeout
            Ok(false)
        },
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let test_passed = run_test(args).await?;

    if test_passed {
        println!("Test passed");
    } else {
        println!("Test failed");
    }

    Ok(())
}
