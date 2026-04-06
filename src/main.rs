mod app;
mod spectrometer;

use anyhow::Result;
use clap::Parser;
use spectrometer::Spectrum;
use std::sync::mpsc;
use std::thread;

#[derive(Parser, Debug)]
#[command(name = "tobes-rs", about = "TorchBearer spectrometer viewer")]
struct Args {
    /// Serial port (e.g. /dev/ttyUSB0, /dev/cu.usbserial-1410).
    /// Omit to auto-select the first available port.
    #[arg(short, long)]
    port: Option<String>,

    /// Run with simulated data — no hardware required
    #[arg(long)]
    demo: bool,

    /// Print available serial ports and exit
    #[arg(long)]
    list_ports: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.list_ports {
        let ports = serialport::available_ports()?;
        if ports.is_empty() {
            println!("No serial ports found.");
        } else {
            println!("Available serial ports:");
            for p in ports {
                println!("  {}", p.port_name);
            }
        }
        return Ok(());
    }

    let (tx, rx) = mpsc::channel::<Spectrum>();

    if args.demo {
        thread::spawn(move || demo_loop(tx));
    } else {
        let port_path = match args.port {
            Some(p) => p,
            None => {
                let ports = serialport::available_ports()?;
                match ports.into_iter().next() {
                    Some(p) => {
                        println!("Auto-selecting port: {}", p.port_name);
                        p.port_name
                    }
                    None => {
                        eprintln!("No serial ports found. Use --port <path> or --demo.");
                        std::process::exit(1);
                    }
                }
            }
        };

        thread::spawn(move || {
            if let Err(e) = spectrometer_loop(port_path, tx) {
                eprintln!("Spectrometer thread error: {e}");
            }
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("tobes-rs")
            .with_inner_size([960.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "tobes-rs",
        options,
        Box::new(move |cc| Ok(Box::new(app::SpectrumApp::new(cc, rx)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Real hardware acquisition loop
// ---------------------------------------------------------------------------

fn spectrometer_loop(port: String, tx: mpsc::Sender<Spectrum>) -> Result<()> {
    let mut spec = spectrometer::TorchBearer::open(&port)?;
    spec.start_streaming()?;

    loop {
        match spec.read_spectrum() {
            Ok(s) => {
                if tx.send(s).is_err() {
                    break; // GUI has exited
                }
            }
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }
    }

    let _ = spec.stop_streaming();
    Ok(())
}

// ---------------------------------------------------------------------------
// Demo loop — generates a fake halogen-like spectrum with a slow drift
// ---------------------------------------------------------------------------

fn demo_loop(tx: mpsc::Sender<Spectrum>) {
    use std::time::{Duration, Instant};

    let mut phase = 0.0f32;

    loop {
        let t0 = Instant::now();

        let wavelengths: Vec<f32> = (340u16..=1000).map(|w| w as f32).collect();
        let intensities: Vec<f32> = wavelengths
            .iter()
            .map(|&wl| {
                // Smooth blackbody-ish curve peaking around 700 nm
                let bb = (-((wl - 700.0) / 220.0).powi(2)).exp();
                // Gentle oscillation for visual feedback
                let wave = (phase + (wl - 340.0) / 100.0).sin() * 0.04;
                (bb * 0.05 + wave).max(0.0)
            })
            .collect();

        let spectrum = Spectrum {
            status:           spectrometer::ExposureStatus::Normal,
            exposure_time_ms: 100.0,
            wavelengths,
            intensities,
            wavelength_start: 340,
            wavelength_end:   1000,
        };

        if tx.send(spectrum).is_err() {
            break;
        }

        phase += 0.08;

        let elapsed = t0.elapsed();
        if elapsed < Duration::from_millis(100) {
            thread::sleep(Duration::from_millis(100) - elapsed);
        }
    }
}
