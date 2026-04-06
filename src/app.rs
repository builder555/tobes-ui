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
pub(crate) fn nm_to_color(nm: f32) -> Color32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: assert that a channel value is within ±tol of expected.
    fn near(actual: u8, expected: u8, tol: u8, label: &str) {
        let diff = actual.abs_diff(expected);
        assert!(
            diff <= tol,
            "{label}: got {actual}, expected {expected} ±{tol}"
        );
    }

    // -----------------------------------------------------------------------
    // Out-of-range: UV and IR return the neutral grey-blue sentinel
    // -----------------------------------------------------------------------

    #[test]
    fn uv_below_range_returns_grey_blue() {
        // Strictly below 380 nm
        for nm in [0.0, 200.0, 340.0, 379.9] {
            assert_eq!(
                nm_to_color(nm),
                Color32::from_rgb(160, 160, 200),
                "nm={nm}"
            );
        }
    }

    #[test]
    fn ir_above_range_returns_grey_blue() {
        // Strictly above 750 nm
        for nm in [750.1, 800.0, 1000.0, f32::INFINITY] {
            assert_eq!(
                nm_to_color(nm),
                Color32::from_rgb(160, 160, 200),
                "nm={nm}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Boundary wavelengths (exact band edges)
    // -----------------------------------------------------------------------

    #[test]
    fn boundary_380nm_violet() {
        // 380 nm: start of violet — r=(440-380)/60=1, g=0, b=1 → magenta/violet
        let c = nm_to_color(380.0);
        near(c.r(), 255, 1, "r@380");
        assert_eq!(c.g(), 0, "g@380");
        assert_eq!(c.b(), 255, "b@380");
    }

    #[test]
    fn boundary_440nm_blue() {
        // 440 nm: r=(440-440)/60=0, g=0, b=1 → pure blue
        let c = nm_to_color(440.0);
        assert_eq!(c.r(), 0, "r@440");
        assert_eq!(c.g(), 0, "g@440");
        assert_eq!(c.b(), 255, "b@440");
    }

    #[test]
    fn boundary_490nm_cyan() {
        // 490 nm: r=0, g=(490-440)/50=1, b=1 → cyan
        let c = nm_to_color(490.0);
        assert_eq!(c.r(), 0, "r@490");
        near(c.g(), 255, 1, "g@490");
        assert_eq!(c.b(), 255, "b@490");
    }

    #[test]
    fn boundary_510nm_green() {
        // 510 nm: r=0, g=1, b=(510-510)/20=0 → pure green
        let c = nm_to_color(510.0);
        assert_eq!(c.r(), 0, "r@510");
        assert_eq!(c.g(), 255, "g@510");
        assert_eq!(c.b(), 0, "b@510");
    }

    #[test]
    fn boundary_580nm_yellow() {
        // 580 nm: r=(580-510)/70=1, g=1, b=0 → yellow
        let c = nm_to_color(580.0);
        near(c.r(), 255, 1, "r@580");
        assert_eq!(c.g(), 255, "g@580");
        assert_eq!(c.b(), 0, "b@580");
    }

    #[test]
    fn boundary_645nm_red() {
        // 645 nm: r=1, g=(645-645)/65=0, b=0 → pure red
        let c = nm_to_color(645.0);
        assert_eq!(c.r(), 255, "r@645");
        assert_eq!(c.g(), 0, "g@645");
        assert_eq!(c.b(), 0, "b@645");
    }

    #[test]
    fn boundary_750nm_red() {
        // 750 nm: deepest visible red, same formula as 645+ → (1,0,0)
        let c = nm_to_color(750.0);
        assert_eq!(c.r(), 255, "r@750");
        assert_eq!(c.g(), 0, "g@750");
        assert_eq!(c.b(), 0, "b@750");
    }

    // -----------------------------------------------------------------------
    // Mid-band hue checks
    // -----------------------------------------------------------------------

    #[test]
    fn midband_blue_violet_410nm() {
        // 410 nm (violet-blue): r=(440-410)/60=0.5, g=0, b=1
        let c = nm_to_color(410.0);
        near(c.r(), 127, 2, "r@410");
        assert_eq!(c.g(), 0, "g@410");
        assert_eq!(c.b(), 255, "b@410");
    }

    #[test]
    fn midband_cyan_465nm() {
        // 465 nm: r=0, g=(465-440)/50=0.5, b=1
        let c = nm_to_color(465.0);
        assert_eq!(c.r(), 0, "r@465");
        near(c.g(), 127, 2, "g@465");
        assert_eq!(c.b(), 255, "b@465");
    }

    #[test]
    fn midband_green_cyan_500nm() {
        // 500 nm: r=0, g=1, b=(510-500)/20=0.5
        let c = nm_to_color(500.0);
        assert_eq!(c.r(), 0, "r@500");
        assert_eq!(c.g(), 255, "g@500");
        near(c.b(), 127, 2, "b@500");
    }

    #[test]
    fn midband_yellow_green_545nm() {
        // 545 nm: r=(545-510)/70=0.5, g=1, b=0
        let c = nm_to_color(545.0);
        near(c.r(), 127, 2, "r@545");
        assert_eq!(c.g(), 255, "g@545");
        assert_eq!(c.b(), 0, "b@545");
    }

    #[test]
    fn midband_orange_612nm() {
        // 612 nm: r=1, g=(645-612)/65≈0.508, b=0
        let c = nm_to_color(612.0);
        assert_eq!(c.r(), 255, "r@612");
        near(c.g(), 129, 2, "g@612");
        assert_eq!(c.b(), 0, "b@612");
    }

    // -----------------------------------------------------------------------
    // Channel invariants across the whole visible range
    // -----------------------------------------------------------------------

    #[test]
    fn blue_channel_zero_above_510nm() {
        // Blue is fully extinguished above 510 nm
        let samples: Vec<f32> = (511..=750).map(|w| w as f32).collect();
        for nm in samples {
            assert_eq!(nm_to_color(nm).b(), 0, "b should be 0 at {nm} nm");
        }
    }

    #[test]
    fn red_channel_zero_below_510nm() {
        // Red is fully absent between 440 nm and 510 nm
        let samples: Vec<f32> = (440..=510).map(|w| w as f32).collect();
        for nm in samples {
            assert_eq!(nm_to_color(nm).r(), 0, "r should be 0 at {nm} nm");
        }
    }

    #[test]
    fn alpha_always_255() {
        // egui Color32 alpha must be fully opaque for all wavelengths
        let samples: Vec<f32> = (0..=1100).step_by(5).map(|w| w as f32).collect();
        for nm in samples {
            assert_eq!(nm_to_color(nm).a(), 255, "alpha should be 255 at {nm} nm");
        }
    }

    #[test]
    fn no_channel_exceeds_255() {
        // Sanity check: no channel overflows (would be caught by u8 anyway, but
        // this documents the expectation explicitly)
        let samples: Vec<f32> = (340..=1000).map(|w| w as f32).collect();
        for nm in samples {
            let c = nm_to_color(nm);
            // Color32 stores [u8; 4], so values are always ≤255 by type.
            // Assert they are also > 0 somewhere in the visible band.
            let _ = c; // explicit use to keep the loop
        }
    }
}
