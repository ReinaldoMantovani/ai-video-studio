//! Representação temporal exata.
//!
//! Todo tempo da timeline é um inteiro em **flicks** (1/705_600_000 s).
//! Essa base divide exatamente 24, 25, 30, 48, 50, 60, 90, 100, 120 fps,
//! os NTSC 23.976/29.97/59.94 (x/1001) e as taxas de áudio 44.1 kHz e 48 kHz.
//! Somas e subtrações nunca acumulam erro de arredondamento; conversões para
//! segundos só acontecem na borda (argumentos do FFmpeg / UI).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use ts_rs::TS;

pub const FLICKS_PER_SECOND: i64 = 705_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export, type = "number")]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);

    pub const fn from_flicks(f: i64) -> Self {
        Time(f)
    }
    pub const fn flicks(self) -> i64 {
        self.0
    }
    pub const fn from_secs(s: i64) -> Self {
        Time(s * FLICKS_PER_SECOND)
    }
    pub const fn from_millis(ms: i64) -> Self {
        Time(ms * (FLICKS_PER_SECOND / 1000))
    }
    /// Converte segundos (f64) arredondando para o flick mais próximo.
    pub fn from_secs_f64(s: f64) -> Self {
        Time((s * FLICKS_PER_SECOND as f64).round() as i64)
    }
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }
    /// Tempo do frame `n` (início do frame) para a taxa `fps`.
    pub fn from_frames(n: i64, fps: Rational) -> Self {
        // n * den / num segundos  => flicks = n * den * F / num
        Time(((n as i128 * fps.den as i128 * FLICKS_PER_SECOND as i128) / fps.num as i128) as i64)
    }
    /// Índice do frame que contém este instante.
    pub fn frame_floor(self, fps: Rational) -> i64 {
        let num = self.0 as i128 * fps.num as i128;
        let den = fps.den as i128 * FLICKS_PER_SECOND as i128;
        num.div_euclid(den) as i64
    }
    /// Arredonda para a borda de frame mais próxima.
    pub fn snap_to_frame(self, fps: Rational) -> Self {
        let num = self.0 as i128 * fps.num as i128;
        let den = fps.den as i128 * FLICKS_PER_SECOND as i128;
        let n = (num + den / 2).div_euclid(den) as i64;
        Time::from_frames(n, fps)
    }
    pub fn max(self, o: Time) -> Time {
        if self >= o {
            self
        } else {
            o
        }
    }
    pub fn min(self, o: Time) -> Time {
        if self <= o {
            self
        } else {
            o
        }
    }
    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
    pub fn abs(self) -> Time {
        Time(self.0.abs())
    }

    /// Formato decimal exato até microssegundos, para argumentos do FFmpeg
    /// (ex.: `12.345678`). Nunca usa notação científica.
    pub fn to_ffmpeg(self) -> String {
        let neg = self.0 < 0;
        let a = (self.0 as i128).abs();
        // arredonda para microssegundos
        let us = (a * 1_000_000 + FLICKS_PER_SECOND as i128 / 2) / FLICKS_PER_SECOND as i128;
        let s = us / 1_000_000;
        let frac = us % 1_000_000;
        format!("{}{}.{:06}", if neg { "-" } else { "" }, s, frac)
    }

    /// Timecode `HH:MM:SS.mmm`.
    pub fn to_timecode(self) -> String {
        let ms = (self.0 as i128 * 1000 / FLICKS_PER_SECOND as i128).max(0) as i64;
        format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
    }

    /// Aceita `SS`, `SS.sss`, `MM:SS(.sss)` e `HH:MM:SS(.sss)`.
    pub fn parse_timecode(s: &str) -> Option<Time> {
        let parts: Vec<&str> = s.trim().split(':').collect();
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        let mut total = 0f64;
        for (i, p) in parts.iter().enumerate() {
            let v: f64 = p.parse().ok()?;
            if v < 0.0 || (i > 0 && v >= 60.0) {
                return None;
            }
            total = total * 60.0 + v;
        }
        Some(Time::from_secs_f64(total))
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, o: Time) -> Time {
        Time(self.0 + o.0)
    }
}
impl Sub for Time {
    type Output = Time;
    fn sub(self, o: Time) -> Time {
        Time(self.0 - o.0)
    }
}
impl AddAssign for Time {
    fn add_assign(&mut self, o: Time) {
        self.0 += o.0
    }
}
impl SubAssign for Time {
    fn sub_assign(&mut self, o: Time) {
        self.0 -= o.0
    }
}
impl std::ops::Mul<i64> for Time {
    type Output = Time;
    fn mul(self, k: i64) -> Time {
        Time(self.0 * k)
    }
}

impl std::ops::Div<i64> for Time {
    type Output = Time;
    fn div(self, k: i64) -> Time {
        Time(self.0 / k)
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(-self.0)
    }
}
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_timecode())
    }
}

/// Número racional positivo (FPS, timebase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Rational { num, den }
    }
    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            0.0
        } else {
            self.num as f64 / self.den as f64
        }
    }
    pub fn is_valid(self) -> bool {
        self.num > 0 && self.den > 0
    }
    /// Lê "30000/1001" ou "25".
    pub fn parse(s: &str) -> Option<Rational> {
        let mut it = s.split('/');
        let n: u32 = it.next()?.trim().parse().ok()?;
        let d: u32 = match it.next() {
            Some(d) => d.trim().parse().ok()?,
            None => 1,
        };
        if n == 0 || d == 0 {
            return None;
        }
        let g = gcd(n, d);
        Some(Rational::new(n / g, d / g))
    }
    pub fn to_ffmpeg(self) -> String {
        format!("{}/{}", self.num, self.den)
    }
    /// Duração de um frame.
    pub fn frame_duration(self) -> Time {
        Time::from_frames(1, self)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{:.3}", self.as_f64())
        }
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Intervalo semiaberto `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TimeRange {
    pub start: Time,
    pub end: Time,
}

impl TimeRange {
    pub fn new(start: Time, end: Time) -> Self {
        TimeRange { start, end }
    }
    pub fn duration(&self) -> Time {
        self.end - self.start
    }
    pub fn intersects(&self, o: &TimeRange) -> bool {
        self.start < o.end && o.start < self.end
    }
    pub fn intersection(&self, o: &TimeRange) -> Option<TimeRange> {
        let s = self.start.max(o.start);
        let e = self.end.min(o.end);
        (s < e).then_some(TimeRange::new(s, e))
    }
    pub fn contains(&self, t: Time) -> bool {
        t >= self.start && t < self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_frames_are_exact() {
        let fps = Rational::new(30000, 1001);
        // 1 hora de frames NTSC somados um a um não deriva.
        let mut acc = Time::ZERO;
        for _ in 0..107_892 {
            acc += fps.frame_duration();
        }
        assert_eq!(acc, Time::from_frames(107_892, fps));
        assert_eq!(acc.frame_floor(fps), 107_892);
    }

    #[test]
    fn common_rates_divide_flicks() {
        for (n, d) in [(24, 1), (25, 1), (30, 1), (50, 1), (60, 1), (24000, 1001), (60000, 1001), (48000, 1), (44100, 1)] {
            let f = Time::from_frames(1, Rational::new(n, d)).0;
            assert_eq!(f as i128 * n as i128, FLICKS_PER_SECOND as i128 * d as i128, "{n}/{d}");
        }
    }

    #[test]
    fn ffmpeg_and_timecode_format() {
        assert_eq!(Time::from_millis(1500).to_ffmpeg(), "1.500000");
        assert_eq!(Time::from_secs(3723).to_timecode(), "01:02:03.000");
        assert_eq!(Time::parse_timecode("1:20").unwrap(), Time::from_secs(80));
        assert_eq!(Time::parse_timecode("01:00:01.5").unwrap(), Time::from_millis(3_601_500));
        assert!(Time::parse_timecode("1:75").is_none());
    }

    #[test]
    fn snap_and_rational() {
        let fps = Rational::parse("30000/1001").unwrap();
        let t = Time::from_secs_f64(1.0);
        let s = t.snap_to_frame(fps);
        assert_eq!(s, Time::from_frames(30, fps));
        assert_eq!(Rational::parse("50/2").unwrap(), Rational::new(25, 1));
    }
}
