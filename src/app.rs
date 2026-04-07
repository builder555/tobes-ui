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
                    // Bars are made slightly wider than 1 nm and given a
                    // matching-colour stroke so adjacent bars blend seamlessly
                    // at any zoom level without visible gaps.
                    plot_ui.polygon(
                        Polygon::new(PlotPoints::new(vec![
                            [wl as f64 - 0.55, 0.0],
                            [wl as f64 - 0.55, intensity as f64],
                            [wl as f64 + 0.55, intensity as f64],
                            [wl as f64 + 0.55, 0.0],
                        ]))
                        .fill_color(color)
                        .stroke(Stroke::new(0.5, color)),
                    );
                }
            });
        });
    }
}

/// Map a wavelength in nm to an approximate visible colour using a lookup
/// table with linear interpolation between colour stops.
///
/// The stops are sampled from a reference spectrum image covering 350–1000 nm.
/// Both ends fade to black. The UV region uses blue-violet (no red channel).
pub(crate) fn nm_to_color(nm: f32) -> Color32 {
    // (wavelength_nm, (r, g, b))
    const STOPS: &[(f32, (u8, u8, u8))] = &[
        (350.0, (  0,   0,   0)),  // black
        (370.0, ( 15,   0,  35)),  // very dark violet
        (385.0, ( 50,   0, 100)),  // dark purple
        (420.0, ( 50,   0, 200)),  // blue-purple
        (450.0, (  0,   0, 230)),  // blue
        (480.0, (  0, 140, 220)),  // cyan-blue
        (510.0, (  0, 220,  50)),  // green
        (545.0, (130, 255,   0)),  // yellow-green
        (568.0, (255, 240,   0)),  // yellow
        (585.0, (255, 165,   0)),  // orange
        (605.0, (255,  70,   0)),  // orange-red
        (635.0, (210,   0,   0)),  // red
        (680.0, (130,   0,   0)),  // dark red
        (750.0, ( 55,   0,   0)),  // very dark red
        (850.0, ( 15,   0,   0)),  // near black
        (1000.0,(  0,   0,   0)),  // black
    ];

    // Clamp to table range.
    if nm <= STOPS[0].0 {
        let (r, g, b) = STOPS[0].1;
        return Color32::from_rgb(r, g, b);
    }
    let last = STOPS[STOPS.len() - 1];
    if nm >= last.0 {
        let (r, g, b) = last.1;
        return Color32::from_rgb(r, g, b);
    }

    // Find the surrounding pair and interpolate.
    let idx = STOPS.partition_point(|&(w, _)| w <= nm) - 1;
    let (w0, (r0, g0, b0)) = STOPS[idx];
    let (w1, (r1, g1, b1)) = STOPS[idx + 1];
    let t = (nm - w0) / (w1 - w0);
    let lerp = |a: u8, b: u8| (a as f32 + t * (b as f32 - a as f32)).round() as u8;
    Color32::from_rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(actual: u8, expected: u8, tol: u8, label: &str) {
        let diff = actual.abs_diff(expected);
        assert!(diff <= tol, "{label}: got {actual}, expected {expected} ±{tol}");
    }

    // -----------------------------------------------------------------------
    // Boundary / clamp behaviour
    // -----------------------------------------------------------------------

    #[test]
    fn below_350nm_is_black() {
        for nm in [0.0f32, 100.0, 300.0, 349.9] {
            let c = nm_to_color(nm);
            assert_eq!(c, Color32::from_rgb(0, 0, 0), "nm={nm}");
        }
    }

    #[test]
    fn at_350nm_is_black() {
        let c = nm_to_color(350.0);
        assert_eq!(c, Color32::from_rgb(0, 0, 0), "nm=350");
    }

    #[test]
    fn at_1000nm_is_black() {
        let c = nm_to_color(1000.0);
        assert_eq!(c, Color32::from_rgb(0, 0, 0), "nm=1000");
    }

    #[test]
    fn above_1000nm_is_black() {
        for nm in [1000.1f32, 1200.0, f32::INFINITY] {
            let c = nm_to_color(nm);
            assert_eq!(c, Color32::from_rgb(0, 0, 0), "nm={nm}");
        }
    }

    // -----------------------------------------------------------------------
    // UV region: blue-violet, NO significant red channel
    // -----------------------------------------------------------------------

    #[test]
    fn uv_370nm_is_dark_violet_no_red() {
        // Stop at 370 nm: (15, 0, 35) — very dark violet, red must be minimal
        let c = nm_to_color(370.0);
        near(c.r(), 15, 3, "r@370");
        assert_eq!(c.g(), 0, "g@370");
        near(c.b(), 35, 3, "b@370");
    }

    #[test]
    fn uv_385nm_is_dark_purple() {
        // Stop: (50, 0, 100)
        let c = nm_to_color(385.0);
        near(c.r(), 50, 3, "r@385");
        assert_eq!(c.g(), 0, "g@385");
        near(c.b(), 100, 3, "b@385");
    }

    #[test]
    fn uv_red_channel_low() {
        // Below 450 nm red should stay low (≤50)
        for nm in (350u32..=449).map(|w| w as f32) {
            assert!(
                nm_to_color(nm).r() <= 55,
                "r too high at {nm} nm: got {}",
                nm_to_color(nm).r()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Key visible stops
    // -----------------------------------------------------------------------

    #[test]
    fn stop_420nm_blue_purple() {
        // Stop: (50, 0, 200)
        let c = nm_to_color(420.0);
        near(c.r(), 50, 3, "r@420");
        assert_eq!(c.g(), 0, "g@420");
        near(c.b(), 200, 3, "b@420");
    }

    #[test]
    fn stop_450nm_blue() {
        // Stop: (0, 0, 230)
        let c = nm_to_color(450.0);
        near(c.r(), 0, 3, "r@450");
        assert_eq!(c.g(), 0, "g@450");
        near(c.b(), 230, 3, "b@450");
    }

    #[test]
    fn stop_480nm_cyan_blue() {
        // Stop: (0, 140, 220)
        let c = nm_to_color(480.0);
        near(c.r(), 0, 3, "r@480");
        near(c.g(), 140, 3, "g@480");
        near(c.b(), 220, 3, "b@480");
    }

    #[test]
    fn stop_510nm_green() {
        // Stop: (0, 220, 50)
        let c = nm_to_color(510.0);
        near(c.r(), 0, 3, "r@510");
        near(c.g(), 220, 3, "g@510");
        near(c.b(), 50, 3, "b@510");
    }

    #[test]
    fn stop_545nm_yellow_green() {
        // Stop: (130, 255, 0)
        let c = nm_to_color(545.0);
        near(c.r(), 130, 3, "r@545");
        near(c.g(), 255, 3, "g@545");
        near(c.b(), 0, 3, "b@545");
    }

    #[test]
    fn stop_568nm_yellow() {
        // Stop: (255, 240, 0)
        let c = nm_to_color(568.0);
        near(c.r(), 255, 3, "r@568");
        near(c.g(), 240, 3, "g@568");
        near(c.b(), 0, 3, "b@568");
    }

    #[test]
    fn stop_585nm_orange() {
        // Stop: (255, 165, 0)
        let c = nm_to_color(585.0);
        near(c.r(), 255, 3, "r@585");
        near(c.g(), 165, 3, "g@585");
        near(c.b(), 0, 3, "b@585");
    }

    #[test]
    fn stop_605nm_orange_red() {
        // Stop: (255, 70, 0)
        let c = nm_to_color(605.0);
        near(c.r(), 255, 3, "r@605");
        near(c.g(), 70, 3, "g@605");
        near(c.b(), 0, 3, "b@605");
    }

    #[test]
    fn stop_635nm_red() {
        // Stop: (210, 0, 0)
        let c = nm_to_color(635.0);
        near(c.r(), 210, 3, "r@635");
        near(c.g(), 0, 3, "g@635");
        near(c.b(), 0, 3, "b@635");
    }

    #[test]
    fn stop_750nm_very_dark_red() {
        // Stop: (55, 0, 0)
        let c = nm_to_color(750.0);
        near(c.r(), 55, 3, "r@750");
        assert_eq!(c.g(), 0, "g@750");
        assert_eq!(c.b(), 0, "b@750");
    }

    // -----------------------------------------------------------------------
    // Monotonic IR fade: red-only, decreasing above 680 nm
    // -----------------------------------------------------------------------

    #[test]
    fn ir_fades_to_black() {
        // Above 680 nm only red is non-zero and it decreases
        let mut prev_r = nm_to_color(680.0).r();
        for nm in (681u32..=1000).map(|w| w as f32) {
            let c = nm_to_color(nm);
            assert_eq!(c.g(), 0, "g should be 0 at {nm} nm");
            assert_eq!(c.b(), 0, "b should be 0 at {nm} nm");
            assert!(c.r() <= prev_r, "r not monotonically decreasing at {nm} nm");
            prev_r = c.r();
        }
    }

    // -----------------------------------------------------------------------
    // Channel invariants
    // -----------------------------------------------------------------------

    #[test]
    fn alpha_always_255() {
        let samples: Vec<f32> = (0..=1100).step_by(5).map(|w| w as f32).collect();
        for nm in samples {
            assert_eq!(nm_to_color(nm).a(), 255, "alpha should be 255 at {nm} nm");
        }
    }

    #[test]
    fn green_zero_outside_visible() {
        // Green channel must be 0 below 450 nm and above 650 nm
        for nm in (350u32..=449).map(|w| w as f32) {
            assert_eq!(nm_to_color(nm).g(), 0, "g should be 0 at {nm} nm");
        }
        for nm in (651u32..=1000).map(|w| w as f32) {
            assert_eq!(nm_to_color(nm).g(), 0, "g should be 0 at {nm} nm");
        }
    }
}
