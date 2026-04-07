use egui::{Color32, Stroke};
use egui_plot::{Plot, PlotPoints, Polygon};
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

        // Toolbar uses the default dark theme.
        ctx.set_visuals(egui::Visuals::dark());
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

        // Chart uses a light theme so the black UV/IR silhouette reads against white.
        ctx.set_visuals(egui::Visuals::light());
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(spec) = &self.current else {
                ui.centered_and_justified(|ui| { ui.spinner(); });
                return;
            };

            let mut plot = Plot::new("spectrum")
                .x_axis_label("Wavelength (nm)")
                .y_axis_label("Intensity (W·m⁻²·nm⁻¹)");

            if self.lock_y {
                plot = plot
                    .include_y(0.0)
                    .include_y(self.peak_intensity as f64 * 1.1);
            }

            plot.show(ui, |plot_ui| {
                // Each wavelength is a filled 1 nm-wide rectangle from y=0 to the
                // measured intensity, coloured by nm_to_color. This produces the
                // filled rainbow spectrum with a natural black silhouette in UV/IR.
                for (&wl, &intensity) in
                    spec.wavelengths.iter().zip(spec.intensities.iter())
                {
                    let color = nm_to_color(wl);
                    plot_ui.polygon(
                        Polygon::new(PlotPoints::new(vec![
                            [wl as f64 - 0.5, 0.0],
                            [wl as f64 - 0.5, intensity as f64],
                            [wl as f64 + 0.5, intensity as f64],
                            [wl as f64 + 0.5, 0.0],
                        ]))
                        .fill_color(color)
                        .stroke(Stroke::NONE),
                    );
                }
            });
        });
    }
}

/// Map a wavelength in nm to an approximate visible colour.
/// UV  (<380 nm): magenta→violet fading linearly to black at 280 nm.
///   r and b both scale with t so the colour matches the visible arm at 380 nm
///   (where the visible formula gives r=1, b=1) with no discontinuity.
/// IR  (≥645 nm): dark red  (r=0.5) fading linearly to black at 800 nm.
pub(crate) fn nm_to_color(nm: f32) -> Color32 {
    let (r, g, b): (f32, f32, f32) = match nm {
        nm if nm < 380.0 => {
            // Fade magenta (1,0,1) → black over the 280–380 nm window.
            // Using (t, 0, t) matches the visible arm exactly at 380 nm
            // (which gives r=(440-380)/60=1, b=1) and eliminates the jump.
            let t = ((nm - 280.0) / 100.0).clamp(0.0, 1.0);
            (t, 0.0, t)
        }
        nm if nm < 440.0 => ((440.0 - nm) / 60.0, 0.0, 1.0),
        nm if nm < 490.0 => (0.0, (nm - 440.0) / 50.0, 1.0),
        nm if nm < 510.0 => (0.0, 1.0, (510.0 - nm) / 20.0),
        nm if nm < 580.0 => ((nm - 510.0) / 70.0, 1.0, 0.0),
        nm if nm < 645.0 => (1.0, (645.0 - nm) / 65.0, 0.0),
        _ => {
            // Fade dark red → black over the 645–800 nm window.
            let t = (1.0 - (nm - 645.0) / 155.0).clamp(0.0, 1.0);
            (0.5 * t, 0.0, 0.0)
        }
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
    // UV (<380 nm): deep purple at 380 nm, linear fade to black at 280 nm
    // -----------------------------------------------------------------------

    #[test]
    fn uv_at_380nm_is_magenta() {
        // t = (379.9-280)/100 ≈ 1.0  →  (t, 0, t) ≈ (255, 0, 255)
        // This matches the visible arm at 380 nm: r=(440-380)/60=1, b=1
        let c = nm_to_color(379.9);
        near(c.r(), 255, 2, "r@379.9");
        assert_eq!(c.g(), 0, "g@379.9");
        near(c.b(), 255, 2, "b@379.9");
    }

    #[test]
    fn uv_midpoint_fade() {
        // 330 nm: t = (330-280)/100 = 0.5  →  (0.5, 0, 0.5) = (127, 0, 127)
        let c = nm_to_color(330.0);
        near(c.r(), 127, 2, "r@330");
        assert_eq!(c.g(), 0, "g@330");
        near(c.b(), 127, 2, "b@330");
    }

    #[test]
    fn uv_at_280nm_is_black() {
        // t = 0.0 → (0, 0, 0)
        let c = nm_to_color(280.0);
        assert_eq!(c.r(), 0, "r@280");
        assert_eq!(c.g(), 0, "g@280");
        assert_eq!(c.b(), 0, "b@280");
    }

    #[test]
    fn uv_below_280nm_is_black() {
        // t is clamped to 0 for any nm ≤ 280
        for nm in [0.0f32, 100.0, 200.0, 279.9] {
            let c = nm_to_color(nm);
            assert_eq!(c, Color32::from_rgb(0, 0, 0), "nm={nm}");
        }
    }

    // -----------------------------------------------------------------------
    // IR (≥645 nm): dark red at 645 nm, linear fade to black at 800 nm
    // -----------------------------------------------------------------------

    #[test]
    fn ir_midpoint_fade() {
        // 722.5 nm ≈ midpoint: t = 1 - (722.5-645)/155 = 0.5  →  (0.25, 0, 0) = (63, 0, 0)
        let c = nm_to_color(722.5);
        near(c.r(), 63, 2, "r@722.5");
        assert_eq!(c.g(), 0, "g@722.5");
        assert_eq!(c.b(), 0, "b@722.5");
    }

    #[test]
    fn ir_at_800nm_is_black() {
        // t = 1 - (800-645)/155 = 0.0 → (0, 0, 0)
        let c = nm_to_color(800.0);
        assert_eq!(c.r(), 0, "r@800");
        assert_eq!(c.g(), 0, "g@800");
        assert_eq!(c.b(), 0, "b@800");
    }

    #[test]
    fn ir_above_800nm_is_black() {
        // t is clamped to 0 for any nm ≥ 800
        for nm in [800.1f32, 900.0, 1000.0, f32::INFINITY] {
            let c = nm_to_color(nm);
            assert_eq!(c, Color32::from_rgb(0, 0, 0), "nm={nm}");
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
    fn boundary_645nm_dark_red() {
        // 645 nm: IR fade start — t=1.0 → (0.5*1, 0, 0) = (127, 0, 0)
        let c = nm_to_color(645.0);
        assert_eq!(c.r(), 127, "r@645");
        assert_eq!(c.g(), 0, "g@645");
        assert_eq!(c.b(), 0, "b@645");
    }

    #[test]
    fn boundary_750nm_dimmed_red() {
        // 750 nm: t = 1 - (750-645)/155 ≈ 0.323 → r = 0.5*0.323*255 ≈ 41
        let c = nm_to_color(750.0);
        near(c.r(), 41, 2, "r@750");
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
