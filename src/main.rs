use clap::Parser;
use tokio::process::Command as TokioCommand;
use serde::Deserialize;
use serde_yaml;
use std::process::Stdio;
use std::path::PathBuf;
use tokio::time::{timeout, Duration};
use tokio::fs::File as TokioFile;
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
    
        let mut child_stdout = child.stdout.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stdout"))?;
        let mut child_stderr = child.stderr.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stderr"))?;
    
        let mut output_file = if selected_service == "espflash" {
            if let Some(output_file_path) = &test_output_file_path {
                Some(TokioFile::create(output_file_path).await?)
            } else {
                None
            }
        } else {
            None
        };

        let mut async_stdout = tokio::io::stdout();
        let mut async_stderr = tokio::io::stderr();

        let stdout_fut = tokio::io::copy(&mut child_stdout, &mut async_stdout);
        let stderr_fut = tokio::io::copy(&mut child_stderr, &mut async_stderr);
    
        let copy_fut = async {
            match tokio::try_join!(stdout_fut, stderr_fut) {
                Ok(_) => Ok::<bool, Box<dyn std::error::Error>>(true),
                Err(e) => Err(e.into())
            }
        };
    
        let scenario_fut = async {
            if let Some(scenario) = &scenario {
                let mut scenario_stdout = child.stdout.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stdout for scenario"))?;
                let mut reader = BufReader::new(&mut scenario_stdout);
        
                for step in scenario.steps.iter() {
                    let mut line = String::new();
                    while reader.read_line(&mut line).await? != 0 {
                        if line.contains(&step.wait_serial) {
                            return Ok(true);  // Scenario success
                        }
                        line.clear();
                    }
                }
            }
            Ok(false)  // Scenario not met, or no scenario provided
        };
    
        tokio::select! {
            result = copy_fut => result,
            result = scenario_fut => result,
    }
};

    match timeout(test_timeout_duration, child_future).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => {
            eprintln!("Test execution timed out after {:?} seconds", test_timeout_duration.as_secs());
            let mut child_guard = shared_child.lock().await; // Use the original Arc
            child_guard.kill().await?;
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
