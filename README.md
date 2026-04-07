# tobes-rs: Totally Bearable Spectrometer UI in Rust

Rust viewer for the TorchBearer spectrometer. Displays a live rainbow-filled spectrum chart via USB-serial.

## Requirements

- Rust
- For hardware mode: TorchBearer spectrometer over USB-serial

## Commands

```sh
# Run with live hardware (auto-detects port - may fail)
cargo run --release

# Run with specific port
cargo run --release -- --port /dev/ttyACM0

# Run with synthetic demo data (no hardware needed)
cargo run -- --demo

# Load and display a saved spectrum JSON (no hardware needed)
cargo run -- --file spectrum.json

# List available serial ports
cargo run -- --list-ports

# Run tests
cargo test

# Build optimised binary
cargo build --release

# Cross-compile for Raspberry Pi (aarch64)
cargo build --release --target aarch64-unknown-linux-gnu
```

### if using mise: task shortcuts

```sh
mise run demo        # --demo mode
mise run run         # release build + auto port
mise run test        # cargo test
mise run build       # release binary
mise run build-pi    # cross-compile for Pi
mise run ports       # list serial ports
```

## JSON format

Spectra can be exported from tobes-ui and loaded with `--file`. Expected shape:

```json
{
  "status": "normal",
  "time": 100.0,
  "spd": { "340": 0.0012, "341": 0.0015, "...": "..." },
  "name": "my spectrum"
}
```
