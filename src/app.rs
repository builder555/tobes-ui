use egui::Color32;
use egui_plot::{Line, Plot, PlotPoints};
use std::sync::mpsc::Receiver;

use crate::spectrometer::{ExposureStatus, Spectrum};

pub struct SpectrumApp {
    rx:            Receiver<Spectrum>,
    current:       Option<Spectrum>,
    peak_intensity: f32,
    lock_y:        bool,
}

impl SpectrumApp {
    pub fn new(_cc: &eframe::CreationContext, rx: Receiver<Spectrum>) -> Self {
        Self {
            rx,
            current:        None,
            peak_intensity: 0.0,
            lock_y:         false,
        }
    }
}

impl eframe::App for SpectrumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain the channel and keep the freshest spectrum
        while let Ok(s) = self.rx.try_recv() {
            let max = s.intensities.iter().cloned().fold(0.0f32, f32::max);
            if max > self.peak_intensity {
                self.peak_intensity = max;
            }
            self.current = Some(s);
        }
        ctx.request_repaint();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("tobes-rs");
                ui.separator();

                if let Some(spec) = &self.current {
                    let (dot, label) = match spec.status {
                        ExposureStatus::Normal => (Color32::GREEN,  "normal"),
                        ExposureStatus::Over   => (Color32::RED,    "over"),
                        ExposureStatus::Under  => (Color32::YELLOW, "under"),
                    };
                    ui.colored_label(dot, format!("● {label}"));
                    ui.separator();
                    ui.label(format!("exp {:.1} ms", spec.exposure_time_ms));
                    ui.separator();
                    ui.label(format!(
                        "{}–{} nm",
                        spec.wavelength_start, spec.wavelength_end
                    ));
                } else {
                    ui.label("waiting for data…");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset peak").clicked() {
                        self.peak_intensity = 0.0;
                    }
                    ui.checkbox(&mut self.lock_y, "Lock Y");
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(spec) = &self.current else {
                ui.centered_and_justified(|ui| { ui.spinner(); });
                return;
            };

            let mut plot = Plot::new("spectrum")
                .x_axis_label("Wavelength (nm)")
                .y_axis_label("Intensity (W·m⁻²·nm⁻¹)")
                .legend(egui_plot::Legend::default());

            if self.lock_y {
                // Keep y range stable at [0, peak * 1.1]
                plot = plot
                    .include_y(0.0)
                    .include_y(self.peak_intensity as f64 * 1.1);
            }

            plot.show(ui, |plot_ui| {
                // Draw the spectrum as rainbow-coloured line segments.
                // Each segment groups consecutive pixels with the same hue bucket,
                // keeping an overlap point so the curve is continuous.
                for (pts, color) in spectrum_lines(spec) {
                    plot_ui.line(
                        Line::new(PlotPoints::new(pts))
                            .color(color)
                            .width(2.0),
                    );
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a list of (points, color) segments covering the full spectrum.
/// Consecutive pixels that map to the same colour bucket are grouped together.
fn spectrum_lines(spec: &Spectrum) -> Vec<(Vec<[f64; 2]>, Color32)> {
    let mut result: Vec<(Vec<[f64; 2]>, Color32)> = Vec::new();
    if spec.wavelengths.is_empty() {
        return result;
    }

    let first_color = nm_to_color(spec.wavelengths[0]);
    let mut seg_pts: Vec<[f64; 2]> = Vec::new();
    let mut seg_col = first_color;

    for (&wl, &intensity) in spec.wavelengths.iter().zip(spec.intensities.iter()) {
        let col = nm_to_color(wl);
        if col != seg_col {
            // Overlap the last point into the new segment for continuity
            if let Some(&last) = seg_pts.last() {
                result.push((std::mem::take(&mut seg_pts), seg_col));
                seg_pts.push(last);
            }
            seg_col = col;
        }
        seg_pts.push([wl as f64, intensity as f64]);
    }
    if !seg_pts.is_empty() {
        result.push((seg_pts, seg_col));
    }
    result
}

/// Map a wavelength in nm to an approximate visible colour.
/// UV (<380 nm) and IR (>750 nm) use a neutral grey-blue.
fn nm_to_color(nm: f32) -> Color32 {
    if nm < 380.0 || nm > 750.0 {
        return Color32::from_rgb(160, 160, 200);
    }
    let (r, g, b): (f32, f32, f32) = if nm < 440.0 {
        ((440.0 - nm) / 60.0, 0.0, 1.0)
    } else if nm < 490.0 {
        (0.0, (nm - 440.0) / 50.0, 1.0)
    } else if nm < 510.0 {
        (0.0, 1.0, (510.0 - nm) / 20.0)
    } else if nm < 580.0 {
        ((nm - 510.0) / 70.0, 1.0, 0.0)
    } else if nm < 645.0 {
        (1.0, (645.0 - nm) / 65.0, 0.0)
    } else {
        (1.0, 0.0, 0.0)
    };
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}
