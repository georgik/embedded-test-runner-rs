use clap::Parser;
use std::{fs, path::Path, env};

/// Rust Test Orchestrator for running and comparing test cases
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Sets the path to the Rust project
    #[arg(short, long)]
    project_path: String,

    /// Directory where output files will be stored
    #[arg(short, long)]
    output_directory: String,

    /// Continue execution even if a test fails
    #[arg(short, long)]
    continue_on_error: bool,
}

struct TestCase {
    file_path: String,
    build_mode: String, // "debug" or "release"
}

use std::process::Command;

impl TestCase {
    fn build(&self, project_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let example_name = Path::new(&self.file_path)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or("Failed to extract example name")?;

        // Print message indicating the start of the build
        println!("Building {} in {} mode...", example_name, self.build_mode);

        let mut command = Command::new("cargo");
        command.args(["build", "--example", example_name])
               .current_dir(project_path); // Set the working directory
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


    fn run(&self, project_path: &Path, output_directory: &Path, continue_on_error: bool) -> Result<bool, Box<dyn std::error::Error>> {
        let example_name = Path::new(&self.file_path)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or("Failed to construct ELF path")?;

        let elf_path = format!(
            "target/riscv32imac-unknown-none-elf/{}/examples/{}",
            if self.build_mode == "debug" { "debug" } else { "release" },
            example_name
        );

        // Ensure the output directory is an absolute path
        let absolute_output_dir = if output_directory.is_absolute() {
            output_directory.to_path_buf()
        } else {
            env::current_dir()?.join(output_directory)
        };
        let tmp_output_dir = absolute_output_dir.join("tmp");
        fs::create_dir_all(&tmp_output_dir)?; // Ensure 'tmp' directory exists
        let serial_log_file = tmp_output_dir.join(format!("{}-{}.txt", example_name, self.build_mode));

        // Constructing the command to run
        let command_args = [
            "--elf", &elf_path,
            "--expect-text", "Backtrace",
            "--expect-text", "ERROR - Not enough memory to allocate",
            "--timeout", "5000",
            // "--timeout-exit-code", "0",
            "--serial-log-file", serial_log_file.to_str().ok_or("Failed to convert path to string")?
        ];
        let command_to_run = format!("wokwi-cli {}", command_args.join(" "));

        // Printing information about the test being run
        println!("Testing {}...", example_name);
        println!("Command: {}", command_to_run);

        let output = Command::new("wokwi-cli")
            .args(&command_args)
            .current_dir(project_path)
            .output()?;

       // Adjust the output path to use the 'tmp' directory
       let tmp_output_dir = output_directory.join("tmp");
       fs::create_dir_all(&tmp_output_dir)?; // Ensure 'tmp' directory exists
       let serial_log_file = tmp_output_dir.join(format!("{}-{}.txt", example_name, self.build_mode));

       // Existing command execution code...

       // Determine the test result and move the file to 'passed' or 'failed' directory
       let test_passed = output.status.success();
       let result_dir = if test_passed { "passed" } else { "failed" };
       let final_output_dir = output_directory.join(result_dir);
       fs::create_dir_all(&final_output_dir)?; // Ensure directory exists
       fs::rename(serial_log_file, final_output_dir.join(format!("{}-{}.txt", example_name, self.build_mode)))?;

       if !test_passed && !continue_on_error {
           return Err(Box::new(std::io::Error::new(
               std::io::ErrorKind::Other,
               "Test failed and continue_on_error is false",
           )));
       }
        // Print the command's stdout to the console
        println!("{}", String::from_utf8(output.stdout.clone())?);

       Ok(test_passed)
   }
}

fn discover_test_cases(path: &Path) -> Vec<TestCase> {
    let mut test_cases = Vec::new();

    // Define the examples directory path
    let examples_path = path.join("examples");

    // Check if the examples directory exists
    if examples_path.is_dir() {
        // Iterate over the entries in the examples directory
        if let Ok(entries) = fs::read_dir(examples_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                // Check if the entry is a file and has a .rs extension
                if path.is_file() && path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs") {
                    // Add the test case for both debug and release builds
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


fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut passed_tests = 0;
    let mut failed_tests = 0;

    let project_path = Path::new(&args.project_path);
    let output_directory = Path::new(&args.output_directory);

    let test_cases = discover_test_cases(&project_path);

    // Print the list of test cases
    println!("Discovered test cases:");
    for test in &test_cases {
        println!("{} - {}", test.file_path, test.build_mode);
    }

    for test in &test_cases {
        test.build(&project_path)?;
    }

    for test in test_cases {
        match test.run(&project_path, &output_directory, args.continue_on_error) {
            Ok(true) => passed_tests += 1,
            Ok(false) => failed_tests += 1,
            Err(e) => {
                println!("Error: {}", e);
                failed_tests += 1;
                if !args.continue_on_error {
                    break;
                }
            }
        }
    }

    // Display the summary
    println!("Test run summary:");
    println!("Passed tests: {}", passed_tests);
    println!("Failed tests: {}", failed_tests);

    Ok(())
}