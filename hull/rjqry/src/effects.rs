//! Portable jQuery 4 effects arithmetic from `src/effects/Tween.js` and
//! `src/effects.js` (`speed`, `fx.speeds`, and the Animation tick calculation).
//!
//! Callers supply values, timestamps and hooks. This module does not install a
//! clock, timer, animation-frame callback, style writer, or dequeue completion
//! callback. JavaScript functions/prototypes are outside the owned Value model.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::Value;
use crate::style::default_unit;

pub const SLOW_DURATION_MS: f64 = 600.0;
pub const FAST_DURATION_MS: f64 = 200.0;
pub const DEFAULT_DURATION_MS: f64 = 400.0;

/// The built-in own named speeds. Numeric durations, even negative values or
/// NaN, are preserved by upstream speed normalization. A Timeline can separately
/// enforce the finite nonnegative domain needed by a caller's animation clock.
/// User-installed speed table entries and prototype properties need the host.
pub fn normalize_duration(duration: &Value, effects_off: bool) -> f64 {
    if effects_off {
        return 0.0;
    }
    if let Value::Number(value) = duration {
        return *value;
    }
    match duration.js_string().as_str() {
        "slow" => SLOW_DURATION_MS,
        "fast" => FAST_DURATION_MS,
        _ => DEFAULT_DURATION_MS,
    }
}

/// Upstream `opt.queue == null || opt.queue === true` rule. False and custom
/// queue values are retained; normalization does not start queue processing.
pub fn normalize_queue(queue: &Value) -> Value {
    match queue {
        Value::Undefined | Value::Null | Value::Bool(true) => Value::String("fx".into()),
        other => other.clone(),
    }
}

/// Data-only portion of normalized speed options. `completion` corresponds to
/// upstream `opt.old`; invoking it and dequeuing are explicitly host operations.
/// `easing` is retained as given; Tween supplies its own default when omitted.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeedOptions {
    pub duration_ms: f64,
    pub easing: Value,
    pub queue: Value,
    pub completion: Value,
    pub extra_options: Vec<(String, Value)>,
}

/// Translates the data-value branches of `jQuery.speed(speed, easing, fn)`.
/// Function-valued overloads and the generated completion wrapper require a
/// binding with actual callable values; no callable placeholder is fabricated.
pub fn normalize_speed(
    speed: &Value,
    easing: Option<&Value>,
    complete: Option<&Value>,
    effects_off: bool,
) -> SpeedOptions {
    let mut duration = Value::Undefined;
    let mut selected_easing = Value::Undefined;
    let mut queue = Value::Undefined;
    let mut completion = Value::Undefined;
    let mut extra_options = Vec::new();
    if matches!(speed, Value::Object(_) | Value::Array(_)) {
        // The shallow jQuery.extend({}, speed) branch skips undefined and guards
        // __proto__. Arrays contribute indexed extra properties only.
        let entries = match speed {
            Value::Object(_) => speed
                .ordered_entries()
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect::<Vec<_>>(),
            Value::Array(values) => values
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), value))
                .collect(),
            _ => unreachable!(),
        };
        for (name, value) in entries {
            if name == "__proto__" || matches!(value, Value::Undefined) {
                continue;
            }
            match name.as_str() {
                "duration" => duration = value.clone(),
                "easing" => selected_easing = value.clone(),
                "queue" => queue = value.clone(),
                "complete" => completion = value.clone(),
                "old" => {} // Upstream overwrites this with opt.complete.
                _ => extra_options.push((name, value.clone())),
            }
        }
    } else {
        duration = speed.clone();
        selected_easing = easing.cloned().unwrap_or(Value::Undefined);
        completion = complete
            .filter(|value| value.truthy())
            .or_else(|| easing.filter(|value| value.truthy()))
            .cloned()
            .unwrap_or(Value::Bool(false));
    }
    SpeedOptions {
        duration_ms: normalize_duration(&duration, effects_off),
        easing: selected_easing,
        queue: normalize_queue(&queue),
        completion,
        extra_options,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Easing {
    Linear,
    #[default]
    Swing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasingError {
    /// The built-in cosine approximation admits normal animation progress
    /// [0,1]. For extrapolation callers can supply their own easing function.
    UnsupportedProgress,
}

impl Easing {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "linear" => Some(Self::Linear),
            "swing" => Some(Self::Swing),
            _ => None,
        }
    }

    pub fn sample(self, progress: f64) -> Result<f64, EasingError> {
        match self {
            Self::Linear => Ok(linear(progress)),
            Self::Swing => swing(progress),
        }
    }
}

pub fn linear(progress: f64) -> f64 {
    progress
}

/// `0.5 - cos(progress * PI) / 2`, with a portable Taylor approximation because
/// core supplies no cosine. On [0,1] the test suite checks absolute error below
/// 2e-15 against the host standard library. This is not a bit-exact libm port.
/// Nonfinite input produces NaN as Math.cos does; finite extrapolation is an
/// explicit unsupported domain rather than an inaccurate range reduction.
pub fn swing(progress: f64) -> Result<f64, EasingError> {
    if !progress.is_finite() {
        return Ok(f64::NAN);
    }
    if !(0.0..=1.0).contains(&progress) {
        return Err(EasingError::UnsupportedProgress);
    }
    let (fraction, sign) = if progress > 0.5 {
        (1.0 - progress, -1.0)
    } else {
        (progress, 1.0)
    };
    let x = fraction * core::f64::consts::PI;
    let squared = x * x;
    let mut term = 1.0;
    let mut cosine = 1.0;
    // Reflection confines x to [0,pi/2]; twelve terms leave truncation far below
    // f64 rounding error throughout this interval.
    for index in 1..=12 {
        term *= -squared / f64::from((2 * index - 1) * (2 * index));
        cosine += term;
    }
    Ok(0.5 - sign * cosine / 2.0)
}

/// Numeric part of jQuery.Tween; `start` comes from an explicit host property
/// read, and the host applies `now` through its property/step hooks after run.
#[derive(Clone, Debug, PartialEq)]
pub struct Tween {
    pub start: f64,
    pub now: f64,
    pub end: f64,
    pub pos: Option<f64>,
    pub duration_ms: f64,
    pub easing: Easing,
    pub unit: String,
}

impl Tween {
    pub fn new(
        property: &str,
        start: f64,
        end: f64,
        duration_ms: f64,
        easing: Option<Easing>,
        unit: Option<&str>,
    ) -> Self {
        let unit = unit
            .filter(|unit| !unit.is_empty())
            .unwrap_or_else(|| default_unit(property));
        Self {
            start,
            now: start,
            end,
            pos: None,
            duration_ms,
            easing: easing.unwrap_or_default(),
            unit: unit.into(),
        }
    }

    /// The source bypasses easing altogether for a zero/NaN duration.
    pub fn run(&mut self, percent: f64) -> Result<&mut Self, EasingError> {
        let eased = if number_is_truthy(self.duration_ms) {
            self.easing.sample(percent)?
        } else {
            percent
        };
        self.update(eased);
        Ok(self)
    }

    /// Custom easing receives the same five arguments as Tween.run's JS call:
    /// percent, elapsed, initial value 0, change 1, and duration.
    pub fn run_with(
        &mut self,
        percent: f64,
        mut easing: impl FnMut(f64, f64, f64, f64, f64) -> f64,
    ) -> &mut Self {
        let eased = if number_is_truthy(self.duration_ms) {
            easing(
                percent,
                self.duration_ms * percent,
                0.0,
                1.0,
                self.duration_ms,
            )
        } else {
            percent
        };
        self.update(eased);
        self
    }

    fn update(&mut self, eased: f64) {
        self.pos = Some(eased);
        self.now = (self.end - self.start) * eased + self.start;
    }
}

fn number_is_truthy(value: f64) -> bool {
    value != 0.0 && !value.is_nan()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameProgress {
    pub percent: f64,
    pub remaining_ms: f64,
}

/// Exact arithmetic from the Animation tick, including JavaScript's
/// `(remaining / duration || 0)` fallback. It deliberately does not invent a
/// pre-start clamp; a timestamp before start can produce negative progress.
pub fn animation_progress(start_ms: f64, duration_ms: f64, now_ms: f64) -> FrameProgress {
    let remaining = start_ms + duration_ms - now_ms;
    let remaining_ms = if remaining.is_nan() {
        f64::NAN
    } else if remaining > 0.0 {
        remaining
    } else {
        0.0
    };
    let ratio = remaining_ms / duration_ms;
    FrameProgress {
        percent: 1.0 - if number_is_truthy(ratio) { ratio } else { 0.0 },
        remaining_ms,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineError {
    NonFiniteTime,
    NegativeDuration,
}

/// A caller-owned clock contract around the source tick arithmetic. Creating or
/// sampling it has no side effects and does not arrange future execution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    start_ms: f64,
    duration_ms: f64,
}

impl Timeline {
    pub fn new(start_ms: f64, duration_ms: f64) -> Result<Self, TimelineError> {
        if !start_ms.is_finite()
            || !duration_ms.is_finite()
            || !(start_ms + duration_ms).is_finite()
        {
            return Err(TimelineError::NonFiniteTime);
        }
        if duration_ms < 0.0 {
            return Err(TimelineError::NegativeDuration);
        }
        Ok(Self {
            start_ms,
            duration_ms,
        })
    }

    pub fn frame(self, now_ms: f64) -> Result<FrameProgress, TimelineError> {
        if !now_ms.is_finite() {
            return Err(TimelineError::NonFiniteTime);
        }
        Ok(animation_progress(self.start_ms, self.duration_ms, now_ms))
    }

    pub fn start_ms(self) -> f64 {
        self.start_ms
    }
    pub fn duration_ms(self) -> f64 {
        self.duration_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn speeds_and_queue_options_follow_upstream_cases() {
        for (input, expected) in [
            (Value::Number(100.0), 100.0),
            (Value::Undefined, 400.0),
            (Value::Null, 400.0),
            (Value::Bool(false), 400.0),
            (Value::String("slow".into()), 600.0),
            (Value::String("fast".into()), 200.0),
            (Value::String("unknown".into()), 400.0),
            (Value::Number(-1.0), -1.0),
        ] {
            assert_eq!(normalize_duration(&input, false), expected);
            assert_eq!(normalize_duration(&input, true), 0.0);
        }
        assert!(normalize_duration(&Value::Number(f64::NAN), false).is_nan());
        for input in [Value::Undefined, Value::Null, Value::Bool(true)] {
            assert_eq!(normalize_queue(&input), Value::String("fx".into()));
        }
        for input in [
            Value::Bool(false),
            Value::String("custom".into()),
            Value::Number(0.0),
        ] {
            assert_eq!(normalize_queue(&input), input);
        }
    }

    #[test]
    fn speed_normalization_preserves_data_options_and_does_not_mutate_input() {
        let input = Value::Object(vec![
            ("duration".into(), Value::String("fast".into())),
            ("queue".into(), Value::Null),
            ("easing".into(), Value::String("linear".into())),
            ("special".into(), Value::Bool(true)),
            ("ignored".into(), Value::Undefined),
            ("__proto__".into(), Value::Bool(true)),
        ]);
        let original = input.clone();
        let options = normalize_speed(&input, None, None, false);
        assert_eq!(options.duration_ms, 200.0);
        assert_eq!(options.queue, Value::String("fx".into()));
        assert_eq!(options.easing, Value::String("linear".into()));
        assert_eq!(options.completion, Value::Undefined);
        assert_eq!(
            options.extra_options,
            vec![("special".into(), Value::Bool(true))]
        );
        assert_eq!(input, original);
        let easing = Value::String("linear".into());
        let options = normalize_speed(&Value::Number(100.0), Some(&easing), None, false);
        assert_eq!(options.easing, easing);
        assert_eq!(options.completion, easing);
        assert_eq!(
            normalize_speed(&Value::Undefined, None, None, false).completion,
            Value::Bool(false)
        );
    }

    #[test]
    fn portable_swing_agrees_with_host_cosine_throughout_the_supported_domain() {
        for index in 0..=10_000 {
            let progress = f64::from(index) / 10_000.0;
            let expected = 0.5 - (progress * core::f64::consts::PI).cos() / 2.0;
            assert!(
                (swing(progress).unwrap() - expected).abs() < 2e-15,
                "{progress}"
            );
            assert_eq!(linear(progress), progress);
        }
        assert_eq!(swing(0.0), Ok(0.0));
        assert_eq!(swing(1.0), Ok(1.0));
        assert_eq!(swing(-0.1), Err(EasingError::UnsupportedProgress));
        assert!(swing(f64::INFINITY).unwrap().is_nan());
    }

    #[test]
    fn tween_sampling_leaves_property_reads_and_writes_to_the_host() {
        let mut tween = Tween::new("width", 10.0, 30.0, 100.0, Some(Easing::Linear), None);
        assert_eq!(tween.now, 10.0);
        assert_eq!(tween.pos, None);
        assert_eq!(tween.unit, "px");
        tween.run(0.25).unwrap();
        assert_eq!(tween.pos, Some(0.25));
        assert_eq!(tween.now, 15.0);
        tween.run_with(0.5, |percent, elapsed, start, change, duration| {
            assert_eq!(
                (percent, elapsed, start, change, duration),
                (0.5, 50.0, 0.0, 1.0, 100.0)
            );
            0.75
        });
        assert_eq!(tween.now, 25.0);
        let mut tween = Tween::new("opacity", 0.0, 1.0, 0.0, None, None);
        tween.run_with(0.3, |_, _, _, _, _| {
            panic!("zero duration must bypass easing")
        });
        assert_eq!(tween.now, 0.3);
        assert_eq!(tween.unit, "");
        let mut tween = Tween::new("width", 0.0, 10.0, 0.0, None, Some("em"));
        tween.run(2.0).unwrap();
        assert_eq!(tween.now, 20.0);
        assert_eq!(tween.unit, "em");
    }

    #[test]
    fn caller_owned_timeline_matches_initial_final_and_zero_duration_ticks() {
        let timeline = Timeline::new(100.0, 200.0).unwrap();
        assert_eq!(
            timeline.frame(100.0).unwrap(),
            FrameProgress {
                percent: 0.0,
                remaining_ms: 200.0
            }
        );
        assert_eq!(
            timeline.frame(150.0).unwrap(),
            FrameProgress {
                percent: 0.25,
                remaining_ms: 150.0
            }
        );
        assert_eq!(
            timeline.frame(400.0).unwrap(),
            FrameProgress {
                percent: 1.0,
                remaining_ms: 0.0
            }
        );
        assert!(timeline.frame(50.0).unwrap().percent < 0.0);
        assert_eq!(
            animation_progress(100.0, 0.0, 100.0),
            FrameProgress {
                percent: 1.0,
                remaining_ms: 0.0
            }
        );
        assert_eq!(
            Timeline::new(0.0, -1.0),
            Err(TimelineError::NegativeDuration)
        );
        assert_eq!(timeline.frame(f64::NAN), Err(TimelineError::NonFiniteTime));
    }
}
