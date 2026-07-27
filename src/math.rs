//! Small dependency-free math utilities used in the real-time path.

use core::ops::{Add, AddAssign, Mul, Sub, SubAssign};

/// Minimal single-precision complex number used by the radix-2 FFT.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex32 {
    /// Real component.
    pub re: f32,
    /// Imaginary component.
    pub im: f32,
}

impl Complex32 {
    /// Creates a complex value.
    #[must_use]
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    /// Squared magnitude.
    #[must_use]
    pub fn norm_sqr(self) -> f32 {
        self.re.mul_add(self.re, self.im * self.im)
    }

    /// Complex conjugate.
    #[must_use]
    pub const fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
}

impl Add for Complex32 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl AddAssign for Complex32 {
    fn add_assign(&mut self, rhs: Self) {
        self.re += rhs.re;
        self.im += rhs.im;
    }
}

impl Sub for Complex32 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.re - rhs.re, self.im - rhs.im)
    }
}

impl SubAssign for Complex32 {
    fn sub_assign(&mut self, rhs: Self) {
        self.re -= rhs.re;
        self.im -= rhs.im;
    }
}

impl Mul for Complex32 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self::new(
            self.re.mul_add(rhs.re, -(self.im * rhs.im)),
            self.re.mul_add(rhs.im, self.im * rhs.re),
        )
    }
}

/// Precomputed in-place radix-2 FFT plan.
#[derive(Clone, Debug)]
pub struct FftPlan {
    size: usize,
    bit_reversed: Vec<usize>,
    twiddles: Vec<Complex32>,
}

impl FftPlan {
    /// Creates a forward FFT plan. `size` must be a power of two.
    #[must_use]
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two() && size >= 2);
        let bits = size.trailing_zeros();
        let bit_reversed = (0..size)
            .map(|index| index.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let twiddles = (0..size / 2)
            .map(|index| {
                let angle = -core::f32::consts::TAU * index as f32 / size as f32;
                Complex32::new(angle.cos(), angle.sin())
            })
            .collect();
        Self {
            size,
            bit_reversed,
            twiddles,
        }
    }

    /// Transform size.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Performs an in-place forward FFT.
    pub fn process(&self, buffer: &mut [Complex32]) {
        assert_eq!(buffer.len(), self.size);
        for index in 0..self.size {
            let other = self.bit_reversed[index];
            if other > index {
                buffer.swap(index, other);
            }
        }

        let mut len = 2;
        while len <= self.size {
            let half = len / 2;
            let twiddle_step = self.size / len;
            for base in (0..self.size).step_by(len) {
                for offset in 0..half {
                    let even = buffer[base + offset];
                    let odd = buffer[base + offset + half] * self.twiddles[offset * twiddle_step];
                    buffer[base + offset] = even + odd;
                    buffer[base + offset + half] = even - odd;
                }
            }
            len *= 2;
        }
    }

    /// Performs an in-place inverse FFT and normalizes the result.
    pub fn process_inverse(&self, buffer: &mut [Complex32]) {
        for value in buffer.iter_mut() {
            *value = value.conj();
        }
        self.process(buffer);
        let scale = 1.0 / self.size as f32;
        for value in buffer.iter_mut() {
            *value = value.conj();
            value.re *= scale;
            value.im *= scale;
        }
    }
}

/// Returns a periodic Hann window.
#[must_use]
pub fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|index| 0.5 - 0.5 * (core::f32::consts::TAU * index as f32 / size as f32).cos())
        .collect()
}

/// Linear interpolation.
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Exponential smoothing coefficient for a time constant in seconds.
#[must_use]
pub fn smoothing_alpha(dt: f32, time_constant: f32) -> f32 {
    if time_constant <= 0.0 {
        1.0
    } else {
        1.0 - (-dt / time_constant).exp()
    }
}

/// Wraps a value to `[0, 1)`.
#[must_use]
pub fn wrap01(value: f32) -> f32 {
    value - value.floor()
}

/// Wraps a cycle difference to `[-0.5, 0.5)`.
#[must_use]
pub fn wrap_signed_cycle(value: f32) -> f32 {
    wrap01(value + 0.5) - 0.5
}

/// Cubic smoothstep.
#[must_use]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 == edge1 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Cosine similarity for equally sized vectors.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot = 0.0;
    let mut aa = 0.0;
    let mut bb = 0.0;
    for (&left, &right) in a.iter().zip(b) {
        dot = left.mul_add(right, dot);
        aa = left.mul_add(left, aa);
        bb = right.mul_add(right, bb);
    }
    if aa <= 1.0e-12 || bb <= 1.0e-12 {
        0.0
    } else {
        (dot / (aa * bb).sqrt()).clamp(-1.0, 1.0)
    }
}

/// Deterministic integer hash suitable for spatial variation.
#[must_use]
pub fn hash_u32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

/// Converts a hash into `[0, 1)`.
#[must_use]
pub fn hash_unit(value: u32) -> f32 {
    (hash_u32(value) as f64 / (u32::MAX as f64 + 1.0)) as f32
}

/// Converts OKLCH to linear-light sRGB, reducing chroma when necessary.
#[must_use]
pub fn oklch_to_linear_srgb(lightness: f32, chroma: f32, hue_turns: f32) -> [f32; 3] {
    let l = lightness.clamp(0.0, 1.0);
    let mut low = 0.0;
    let mut high = chroma.max(0.0);
    let mut best = oklab_to_linear_srgb(l, 0.0, 0.0);
    for _ in 0..10 {
        let c = (low + high) * 0.5;
        let angle = core::f32::consts::TAU * wrap01(hue_turns);
        let rgb = oklab_to_linear_srgb(l, c * angle.cos(), c * angle.sin());
        if rgb.iter().all(|channel| *channel >= 0.0 && *channel <= 1.0) {
            best = rgb;
            low = c;
        } else {
            high = c;
        }
    }
    best
}

fn oklab_to_linear_srgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let ll = l_ * l_ * l_;
    let mm = m_ * m_ * m_;
    let ss = s_ * s_ * s_;
    [
        4.076_741_7 * ll - 3.307_711_6 * mm + 0.230_969_94 * ss,
        -1.268_438 * ll + 2.609_757_4 * mm - 0.341_319_4 * ss,
        -0.004_196_086_3 * ll - 0.703_418_6 * mm + 1.707_614_7 * ss,
    ]
}

/// Applies a conventional power-law output transfer function.
#[must_use]
pub fn gamma_encode(linear: f32, gamma: f32) -> f32 {
    linear.clamp(0.0, 1.0).powf(1.0 / gamma.max(0.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_round_trip() {
        let plan = FftPlan::new(16);
        let original: Vec<_> = (0..16)
            .map(|index| Complex32::new((index as f32 * 0.7).sin(), 0.0))
            .collect();
        let mut transformed = original.clone();
        plan.process(&mut transformed);
        plan.process_inverse(&mut transformed);
        for (expected, actual) in original.iter().zip(&transformed) {
            assert!((expected.re - actual.re).abs() < 1.0e-4);
            assert!(actual.im.abs() < 1.0e-4);
        }
    }

    #[test]
    fn oklch_is_in_gamut() {
        for hue in 0..24 {
            let rgb = oklch_to_linear_srgb(0.7, 0.3, hue as f32 / 24.0);
            assert!(rgb.iter().all(|value| (0.0..=1.0).contains(value)));
        }
    }
}
