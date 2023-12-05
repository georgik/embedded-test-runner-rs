use clap::Parser;
use tokio::process::Command as TokioCommand;
use std::process::Stdio;
use std::path::PathBuf;
use tokio::time::{timeout, Duration};
use tokio::fs::File as TokioFile;

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

async fn run_test(args: Args) -> Result<bool, Box<dyn std::error::Error>> {
    let elf_file_path = args.elf_path;
    let selected_service = args.service;
    let test_timeout_duration = args.timeout.map(Duration::from_secs);
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

            if let Some(timeout) = test_timeout_duration {
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

    let mut child = command_builder.spawn()?;
    let mut child_stdout = child.stdout.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stdout"))?;
    let mut child_stderr = child.stderr.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stderr"))?;

    let child_future = async {
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

        tokio::try_join!(stdout_fut, stderr_fut)?;

        if let Some(mut file) = output_file {
            child_stdout = child.stdout.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stdout"))?;
            child_stderr = child.stderr.take().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Failed to take stderr"))?;
            tokio::io::copy(&mut child_stdout, &mut file).await?;
            tokio::io::copy(&mut child_stderr, &mut file).await?;
        }

        child.wait_with_output().await
    };

    match timeout(test_timeout_duration, child_future).await {
        Ok(output_result) => output_result.map(|output| output.status.success()).map_err(Into::into),
        Err(_) => {
            eprintln!("Test execution timed out after {:?} seconds", test_timeout_duration.as_secs());
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
