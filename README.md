# Embedded Test Runner

1. The test runner discovers all examples in a specific projects.
2. Build examples in release and debug mode
3. Lanuch wokwi-cli to run the simulation and stores the result into output directory

## Example of usage

```
cargo run -- --project-path ../esp32-memory-limit --output-directory out --continue-on-error -j 8
```

Results will be stored in directories: `out/passed`, `out/failed`

## Command line parameters

```
Usage: memory-test-runner [OPTIONS] --project-path <PROJECT_PATH> --output-directory <OUTPUT_DIRECTORY>

Options:
  -p, --project-path <PROJECT_PATH>          Sets the path to the Rust project
  -o, --output-directory <OUTPUT_DIRECTORY>  Directory where output files will be stored
  -c, --continue-on-error                    Continue execution even if a test fails
  -j, --parallelism <PARALLELISM>            [default: 0] = CPUs
  -h, --help                                 Print help
  -V, --version                              Print version
```