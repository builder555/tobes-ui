mod app;
mod spectrometer;

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use spectrometer::{ExposureStatus, Spectrum};
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

#[derive(Parser, Debug)]
#[command(name = "tobes-rs", about = "TorchBearer spectrometer viewer")]
struct Args {
    /// Serial port (e.g. /dev/ttyUSB0, /dev/cu.usbserial-1410).
    /// Omit to auto-select the first available port.
    #[arg(short, long)]
    port: Option<String>,

    /// Load a spectrum from a JSON file and display it (no hardware required).
    /// The file must follow the tobes-ui JSON export format.
    #[arg(short, long)]
    file: Option<String>,

    /// Run with synthetic data — no hardware or file required
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

    if let Some(path) = args.file {
        let spectrum = load_json(&path)?;
        thread::spawn(move || file_loop(spectrum, tx));
    } else if args.demo {
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
                        eprintln!("No serial ports found. Use --port <path>, --file <path>, or --demo.");
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
// JSON file loading
// ---------------------------------------------------------------------------

/// Mirrors the subset of fields written by tobes-ui's Spectrum.to_json().
#[derive(Deserialize)]
struct SpectrumJson {
    #[serde(default)]
    status: String,
    time: f32,
    /// wavelength (nm, as string key) → intensity
    spd: HashMap<String, f64>,
    #[serde(default)]
    name: Option<String>,
}

fn load_json(path: &str) -> Result<Spectrum> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read {path}"))?;
    let data: SpectrumJson = serde_json::from_str(&content)
        .with_context(|| format!("Cannot parse {path} as spectrum JSON"))?;

    // Sort by wavelength so the display is left-to-right.
    let mut pairs: Vec<(u16, f32)> = data
        .spd
        .iter()
        .filter_map(|(k, &v)| k.parse::<u16>().ok().map(|wl| (wl, v as f32)))
        .collect();
    pairs.sort_by_key(|&(wl, _)| wl);

    let wavelengths: Vec<f32> = pairs.iter().map(|&(wl, _)| wl as f32).collect();
    let intensities: Vec<f32> = pairs.iter().map(|&(_, i)| i).collect();

    let start = pairs.first().map(|&(wl, _)| wl).unwrap_or(340);
    let end   = pairs.last().map(|&(wl, _)| wl).unwrap_or(1000);

    let status = match data.status.to_lowercase().as_str() {
        "over"  => ExposureStatus::Over,
        "under" => ExposureStatus::Under,
        _       => ExposureStatus::Normal,
    };

    // Use the file name (without extension) as a label shown in the toolbar.
    let label = data.name.unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });
    eprintln!("Loaded: {label}  ({} points, {start}–{end} nm)", wavelengths.len());

    Ok(Spectrum {
        status,
        exposure_time_ms: data.time,
        wavelengths,
        intensities,
        wavelength_start: start,
        wavelength_end:   end,
    })
}

// ---------------------------------------------------------------------------
// File replay loop — sends the same spectrum at display rate
// ---------------------------------------------------------------------------

fn file_loop(spectrum: Spectrum, tx: mpsc::Sender<Spectrum>) {
    loop {
        if tx.send(spectrum.clone()).is_err() {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
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
                    break;
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
// Demo loop — synthetic blackbody-ish curve with slow drift
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
                let bb   = (-((wl - 700.0) / 220.0).powi(2)).exp();
                let wave = (phase + (wl - 340.0) / 100.0).sin() * 0.04;
                (bb * 0.05 + wave).max(0.0)
            })
            .collect();

        let spectrum = Spectrum {
            status:           ExposureStatus::Normal,
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
