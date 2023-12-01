# Embedded Test Runner

1. The test runner discovers all examples in a specific projects.
2. Build examples in release and debug mode
3. Lanuch wokwi-cli to run the simulation and stores the result into output directory

## Example of usage

```
cargo run -- --project-path ../esp32-memory-limit --output-directory out
```
