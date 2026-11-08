// Copyright (c) 2026 Replexo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Colour adjustments for the Adjust view
//!
//! Brightness, contrast, highlights and shadows form one tone curve that is
//! applied to each colour channel. Saturation is applied afterwards as a
//! linear mix with the pixel's luma.
//!
//! The live preview uses GTK render nodes: brightness is a gamma transfer,
//! shadows and highlights a small table transfer, contrast a linear transfer
//! and saturation a colour matrix. Only the table is approximate. Saving
//! applies the exact curve to the pixels.

/// Number of points in the preview's shadows and highlights table
///
/// GTK 4.24 renders the blue channel wrong when the tables of all channels
/// together have more than about 32 points.
pub const TABLE_SIZE: usize = 8;

/// Rec. 709 luma coefficients
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Adjustment values, each in `-1.0..=1.0` with `0.0` meaning unchanged
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Adjustments {
    pub brightness: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
}

/// The adjustable properties, in the order they are shown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    Brightness,
    Contrast,
    Highlights,
    Shadows,
    Saturation,
}

impl Property {
    pub const ALL: [Property; 5] = [
        Self::Brightness,
        Self::Contrast,
        Self::Highlights,
        Self::Shadows,
        Self::Saturation,
    ];
}

impl Adjustments {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    pub fn get(&self, property: Property) -> f32 {
        match property {
            Property::Brightness => self.brightness,
            Property::Contrast => self.contrast,
            Property::Highlights => self.highlights,
            Property::Shadows => self.shadows,
            Property::Saturation => self.saturation,
        }
    }

    pub fn set(&mut self, property: Property, value: f32) {
        let value = value.clamp(-1., 1.);
        match property {
            Property::Brightness => self.brightness = value,
            Property::Contrast => self.contrast = value,
            Property::Highlights => self.highlights = value,
            Property::Shadows => self.shadows = value,
            Property::Saturation => self.saturation = value,
        }
    }

    /// Tone curve for one channel value in `0.0..=1.0`
    ///
    /// Brightness is a gamma curve, which keeps black and white in place.
    /// Shadows and highlights then move the dark or bright part of the range
    /// towards black or white. Contrast stretches the range around the
    /// middle.
    pub fn tone(&self, v: f32) -> f32 {
        let v = v.clamp(0., 1.).powf(self.brightness_exponent());
        let v = self.shadows_highlights(v);
        let (slope, offset) = self.contrast_line();
        (v * slope + offset).clamp(0., 1.)
    }

    /// Exponent of the brightness gamma curve
    pub fn brightness_exponent(&self) -> f32 {
        2_f32.powf(-self.brightness)
    }

    /// Shadows and highlights part of the tone curve
    ///
    /// The shadows bump `v(1 - v)²` peaks at a third, the highlights bump
    /// `v²(1 - v)` at two thirds. With both amounts in `-1..=1` the curve
    /// stays monotonic and inside `0..=1`: at the extremes it is `2v - v²`
    /// or `v²`.
    pub fn shadows_highlights(&self, v: f32) -> f32 {
        let inv = 1. - v;
        let v = v + self.shadows * v * inv * inv + self.highlights * v * v * inv;
        v.clamp(0., 1.)
    }

    /// Contrast as `v * slope + offset`, pivoting on the middle
    pub fn contrast_line(&self) -> (f32, f32) {
        let slope = if self.contrast >= 0. {
            1. + 2. * self.contrast
        } else {
            1. + self.contrast
        };
        (slope, 0.5 - 0.5 * slope)
    }

    /// Shadows and highlights sampled at [`TABLE_SIZE`] points for the preview
    pub fn shadows_highlights_table(&self) -> [f32; TABLE_SIZE] {
        std::array::from_fn(|i| self.shadows_highlights(i as f32 / (TABLE_SIZE - 1) as f32))
    }

    /// The tone curve as the preview renders it, for tests
    #[cfg(test)]
    fn preview_tone(&self, v: f32) -> f32 {
        let v = v.powf(self.brightness_exponent());
        let table = self.shadows_highlights_table();
        let position = v * (TABLE_SIZE - 1) as f32;
        let index = (position as usize).min(TABLE_SIZE - 2);
        let t = position - index as f32;
        let v = table[index] * (1. - t) + table[index + 1] * t;
        let (slope, offset) = self.contrast_line();
        (v * slope + offset).clamp(0., 1.)
    }

    /// Tone curve as an 8-bit lookup table for saving
    pub fn tone_lut(&self) -> [u8; 256] {
        std::array::from_fn(|i| (self.tone(i as f32 / 255.) * 255.).round() as u8)
    }

    /// Saturation as a row-major 3×3 matrix acting on RGB
    pub fn saturation_matrix(&self) -> [[f32; 3]; 3] {
        let s = 1. + self.saturation;
        std::array::from_fn(|row| {
            std::array::from_fn(|col| {
                let identity = if row == col { 1. } else { 0. };
                LUMA[col] * (1. - s) + identity * s
            })
        })
    }

    /// Applies all adjustments to RGBA8 pixels (not premultiplied) in place
    ///
    /// Alpha stays unchanged.
    pub fn apply_rgba8(&self, pixels: &mut [u8]) {
        if self.is_identity() {
            return;
        }

        let lut = self.tone_lut();
        let matrix = self.saturation_matrix();
        let with_saturation = self.saturation != 0.;

        for pixel in pixels.chunks_exact_mut(4) {
            let toned = [
                lut[pixel[0] as usize],
                lut[pixel[1] as usize],
                lut[pixel[2] as usize],
            ];

            if with_saturation {
                let rgb = toned.map(|x| x as f32);
                for (channel, row) in matrix.iter().enumerate() {
                    let value = row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2];
                    pixel[channel] = value.round().clamp(0., 255.) as u8;
                }
            } else {
                pixel[..3].copy_from_slice(&toned);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(property: Property, value: f32) -> Adjustments {
        let mut adjustments = Adjustments::default();
        adjustments.set(property, value);
        adjustments
    }

    fn samples() -> impl Iterator<Item = f32> {
        (0..=1000).map(|i| i as f32 / 1000.)
    }

    #[test]
    fn identity_changes_nothing() {
        let adjustments = Adjustments::default();
        assert!(adjustments.is_identity());
        for v in samples() {
            assert!((adjustments.tone(v) - v).abs() < 1e-6);
        }
        assert_eq!(adjustments.tone_lut(), std::array::from_fn(|i| i as u8));

        let mut pixels: Vec<u8> = (0..=255).flat_map(|v| [v, 255 - v, v / 2, 200]).collect();
        let before = pixels.clone();
        adjustments.apply_rgba8(&mut pixels);
        assert_eq!(pixels, before);
    }

    #[test]
    fn tone_stays_in_range_and_monotonic() {
        for property in [
            Property::Brightness,
            Property::Contrast,
            Property::Highlights,
            Property::Shadows,
        ] {
            for value in [-1., -0.5, -0.1, 0.1, 0.5, 1.] {
                let adjustments = with(property, value);
                let mut previous = 0.;
                for v in samples() {
                    let t = adjustments.tone(v);
                    assert!((0. ..=1.).contains(&t), "{property:?} {value} {v} -> {t}");
                    assert!(
                        t >= previous - 1e-6,
                        "{property:?} {value} not monotonic at {v}"
                    );
                    previous = t;
                }
            }
        }
    }

    #[test]
    fn brightness_keeps_black_and_white() {
        for value in [-1., 1.] {
            let adjustments = with(Property::Brightness, value);
            assert_eq!(adjustments.tone(0.), 0.);
            assert_eq!(adjustments.tone(1.), 1.);
        }
        assert!(with(Property::Brightness, 0.5).tone(0.5) > 0.5);
        assert!(with(Property::Brightness, -0.5).tone(0.5) < 0.5);
    }

    #[test]
    fn contrast_pivots_on_middle() {
        let more = with(Property::Contrast, 0.5);
        let less = with(Property::Contrast, -0.5);
        assert!((more.tone(0.5) - 0.5).abs() < 1e-6);
        assert!(more.tone(0.25) < 0.25 && more.tone(0.75) > 0.75);
        assert!(less.tone(0.25) > 0.25 && less.tone(0.75) < 0.75);
        // Full negative contrast is flat grey
        assert!((with(Property::Contrast, -1.).tone(0.9) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shadows_and_highlights_mostly_touch_their_side() {
        let shadows = with(Property::Shadows, 1.);
        let lift = |v: f32| shadows.tone(v) - v;
        assert!(lift(0.2) > 0.1);
        assert!(lift(0.9) < 0.01);
        assert!(lift(0.2) > 3. * lift(0.8));
        assert_eq!(shadows.tone(0.), 0.);
        assert_eq!(shadows.tone(1.), 1.);

        let highlights = with(Property::Highlights, -1.);
        let drop = |v: f32| v - highlights.tone(v);
        assert!(drop(0.8) > 0.1);
        assert!(drop(0.1) < 0.01);
        assert!(drop(0.8) > 3. * drop(0.2));
        assert_eq!(highlights.tone(0.), 0.);
        assert_eq!(highlights.tone(1.), 1.);
    }

    #[test]
    fn shadows_and_highlights_combined_stay_monotonic() {
        for shadows in [-1., 1.] {
            for highlights in [-1., 1.] {
                let adjustments = Adjustments {
                    shadows,
                    highlights,
                    ..Default::default()
                };
                let mut previous = 0.;
                for v in samples() {
                    let t = adjustments.tone(v);
                    assert!(t >= previous - 1e-6 && (0. ..=1.).contains(&t));
                    previous = t;
                }
            }
        }
    }

    #[test]
    fn saturation_keeps_grey_and_luma() {
        for value in [-1., -0.3, 0.4, 1.] {
            let adjustments = with(Property::Saturation, value);
            let mut grey = vec![128, 128, 128, 255];
            adjustments.apply_rgba8(&mut grey);
            assert_eq!(grey, [128, 128, 128, 255]);

            // Each matrix row sums to one, so luma is preserved
            let m = adjustments.saturation_matrix();
            for row in m {
                assert!((row.iter().sum::<f32>() - 1.).abs() < 1e-5);
            }
        }

        let mut red = vec![200, 50, 50, 255];
        with(Property::Saturation, -1.).apply_rgba8(&mut red);
        assert!(red[0] == red[1] && red[1] == red[2], "{red:?} is not grey");

        let mut red = vec![200, 50, 50, 255];
        with(Property::Saturation, 0.5).apply_rgba8(&mut red);
        assert!(red[0] > 200 && red[1] < 50);
    }

    #[test]
    fn alpha_is_untouched() {
        let mut adjustments = Adjustments::default();
        Property::ALL.iter().for_each(|p| adjustments.set(*p, 0.7));
        let mut pixels = vec![10, 120, 240, 0, 90, 90, 90, 77];
        adjustments.apply_rgba8(&mut pixels);
        assert_eq!(pixels[3], 0);
        assert_eq!(pixels[7], 77);
    }

    #[test]
    fn preview_follows_tone_curve() {
        for (shadows, highlights) in [
            (0., 0.),
            (1., 0.),
            (-1., 0.),
            (0., 1.),
            (0., -1.),
            (0.7, -0.6),
        ] {
            let adjustments = Adjustments {
                brightness: 0.4,
                contrast: 0.3,
                shadows,
                highlights,
                ..Default::default()
            };
            for v in samples() {
                let difference = (adjustments.preview_tone(v) - adjustments.tone(v)).abs();
                assert!(difference < 0.03, "{adjustments:?} at {v}: {difference}");
            }
        }
    }

    #[test]
    fn set_clamps() {
        let mut adjustments = Adjustments::default();
        adjustments.set(Property::Contrast, 3.);
        assert_eq!(adjustments.contrast, 1.);
        adjustments.set(Property::Contrast, -3.);
        assert_eq!(adjustments.contrast, -1.);
    }
}
