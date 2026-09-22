// CamillaDSP - A flexible tool for processing audio
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

//! Float types that cannot hold `NaN` or an infinity.
//!
//! No config value has a meaningful `.nan` or `.inf` setting, and letting one in is worse than
//! it looks: the range tests in the validators are written as "reject if bad" (`<= 0.0` and
//! similar), and every comparison against NaN is false, so a NaN passes all of them. An infinity
//! passes any test that is bounded on one side only.
//!
//! Putting the rule in the type rather than in a `deserialize_with` on each field means a new
//! float field in the config cannot forget it, and the contract is visible in the field
//! declaration. Deserializing rejects a non-finite value with the line and column it came from.

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::ops::Deref;

macro_rules! finite_float {
    ($name:ident, $prim:ty, $doc:literal) => {
        #[doc = $doc]
        ///
        /// Construct one with [`new`](Self::new), which rejects a non-finite value, and read the
        /// wrapped number with [`get`](Self::get) or by dereferencing. Arithmetic deliberately
        /// needs the value taken out first, since the result of a division could be non-finite
        /// and would no longer honour the guarantee this type makes.
        #[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name($prim);

        impl $name {
            /// Wrap a value, or return `None` if it is `NaN` or an infinity.
            ///
            /// The derived `Default` is zero, which is finite, so it upholds the guarantee.
            pub fn new(value: $prim) -> Option<Self> {
                value.is_finite().then_some(Self(value))
            }

            /// The wrapped value, always finite.
            pub const fn get(self) -> $prim {
                self.0
            }

            /// Wrap a value that is already known to be finite.
            ///
            /// For code inside CamillaDSP that builds config values rather than parsing them,
            /// such as the combo filters generating their biquad sections from constants. A
            /// config read from a file never takes this path, it goes through `Deserialize`,
            /// which reports a config error instead of panicking.
            ///
            /// # Panics
            ///
            /// If the value is `NaN` or an infinity, which would mean a bug in the caller.
            #[track_caller]
            pub fn expect_finite(value: $prim) -> Self {
                Self::new(value).expect("value should be finite")
            }
        }

        /// Reads and method calls go straight through to the wrapped number.
        impl Deref for $name {
            type Target = $prim;

            fn deref(&self) -> &$prim {
                &self.0
            }
        }

        impl From<$name> for $prim {
            fn from(value: $name) -> $prim {
                value.0
            }
        }

        impl TryFrom<$prim> for $name {
            type Error = NotFinite;

            fn try_from(value: $prim) -> Result<Self, NotFinite> {
                Self::new(value).ok_or(NotFinite)
            }
        }

        /// Comparing against a bare number keeps the range tests in the validators readable.
        impl PartialEq<$prim> for $name {
            fn eq(&self, other: &$prim) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for $prim {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl PartialOrd<$prim> for $name {
            fn partial_cmp(&self, other: &$prim) -> Option<Ordering> {
                self.0.partial_cmp(other)
            }
        }

        impl PartialOrd<$name> for $prim {
            fn partial_cmp(&self, other: &$name) -> Option<Ordering> {
                self.partial_cmp(&other.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = <$prim>::deserialize(deserializer)?;
                Self::new(value).ok_or_else(|| {
                    de::Error::invalid_value(
                        de::Unexpected::Float(value as f64),
                        &"a finite number",
                    )
                })
            }
        }
    };
}

/// Returned when a conversion into a finite float is given `NaN` or an infinity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotFinite;

impl fmt::Display for NotFinite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("value must be a finite number")
    }
}

impl std::error::Error for NotFinite {}

finite_float!(
    FiniteF64,
    f64,
    "An `f64` that is guaranteed to be neither `NaN` nor an infinity."
);
finite_float!(
    FiniteF32,
    f32,
    "An `f32` that is guaranteed to be neither `NaN` nor an infinity."
);

/// Shorthand for [`FiniteF64::expect_finite`], for building config values in tests and in the
/// filter builders that generate sections from constants.
macro_rules! finite {
    ($value:expr) => {
        $crate::config::FiniteF64::expect_finite($value)
    };
}

/// The [`FiniteF32`] counterpart of [`finite!`]. Only the tests build `f32` config values
/// directly, everything else reads them through a getter.
#[cfg(test)]
macro_rules! finite32 {
    ($value:expr) => {
        $crate::config::FiniteF32::expect_finite($value)
    };
}

pub(crate) use finite;
#[cfg(test)]
pub(crate) use finite32;

#[cfg(test)]
mod tests {
    use super::{FiniteF32, FiniteF64};

    #[test]
    fn rejects_non_finite() {
        assert!(FiniteF64::new(1.5).is_some());
        assert!(FiniteF64::new(0.0).is_some());
        assert!(FiniteF64::new(-1.0e300).is_some());
        assert!(FiniteF64::new(f64::NAN).is_none());
        assert!(FiniteF64::new(f64::INFINITY).is_none());
        assert!(FiniteF64::new(f64::NEG_INFINITY).is_none());
        assert!(FiniteF32::new(1.5).is_some());
        assert!(FiniteF32::new(f32::NAN).is_none());
        assert!(FiniteF32::new(f32::INFINITY).is_none());
    }

    #[test]
    fn compares_against_bare_numbers() {
        let value = FiniteF64::new(2.0).unwrap();
        assert!(value > 1.0);
        assert!(value <= 2.0);
        assert!(value == 2.0);
        assert!(1.0 < value);
        // The validators are full of tests in this shape, and they still read the same way.
        assert!(value > 0.0);
    }

    #[test]
    fn derefs_to_the_wrapped_number() {
        let value = FiniteF64::new(-3.5).unwrap();
        assert_eq!(value.abs(), 3.5);
        assert_eq!(*value - 1.0, -4.5);
        assert_eq!(value.get(), -3.5);
        assert_eq!(f64::from(value), -3.5);
    }

    #[test]
    fn deserializes_transparently() {
        let good: FiniteF64 = yaml_serde::from_str("1.25").unwrap();
        assert_eq!(good.get(), 1.25);
        assert!(yaml_serde::from_str::<FiniteF64>(".nan").is_err());
        assert!(yaml_serde::from_str::<FiniteF64>(".inf").is_err());
        assert!(yaml_serde::from_str::<FiniteF64>("-.inf").is_err());
        assert!(yaml_serde::from_str::<FiniteF32>(".nan").is_err());
    }

    #[test]
    fn serializes_as_a_plain_number() {
        let value = FiniteF64::new(0.5).unwrap();
        assert_eq!(yaml_serde::to_string(&value).unwrap().trim(), "0.5");
    }
}
