// Copyright 2021-2023 Protocol Labs
// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use std::cell::{Cell, RefCell};
use std::fmt::{Debug, Display};
use std::ops::{Add, AddAssign, Mul, Sub, SubAssign};

use num_traits::Zero;

pub use self::charge::GasCharge;
pub(crate) use self::outputs::GasOutputs;
pub use self::price_list::{price_list_by_network_version, PriceList, WasmGasPrices};
pub use self::timer::{GasInstant, GasTimer};
use crate::kernel::{ExecutionError, Result};

mod charge;
mod outputs;
mod price_list;
mod timer;

pub const MILLIGAS_PRECISION: i64 = 1000;

/// A typesafe representation of gas (internally stored as milligas).
///
/// - All math operations are _saturating_ and never overflow.
/// - Enforces correct units by making it impossible to, e.g., get gas squared (by multiplying gas
///   by gas).
/// - Makes it harder to confuse gas and milligas.
#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Copy, Clone, Default)]
pub struct Gas(i64 /* milligas */);

impl Debug for Gas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 == 0 {
            f.debug_tuple("Gas").field(&0 as &dyn Debug).finish()
        } else {
            let integral = self.0 / MILLIGAS_PRECISION;
            let fractional = self.0 % MILLIGAS_PRECISION;
            f.debug_tuple("Gas")
                .field(&format_args!("{integral}.{fractional:03}"))
                .finish()
        }
    }
}

impl Display for Gas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 == 0 {
            f.write_str("0")
        } else {
            let integral = self.0 / MILLIGAS_PRECISION;
            let fractional = self.0 % MILLIGAS_PRECISION;
            write!(f, "{integral}.{fractional:03}")
        }
    }
}

impl Gas {
    /// Construct a `Gas` from milligas.
    #[inline]
    pub const fn from_milligas(milligas: i64) -> Gas {
        Gas(milligas)
    }

    /// Construct a `Gas` from gas, scaling up. If this exceeds the width of an i64, it saturates at
    /// `i64::MAX` milligas.
    #[inline]
    pub const fn new(gas: i64) -> Gas {
        Gas(gas.saturating_mul(MILLIGAS_PRECISION))
    }

    #[inline]
    pub const fn is_saturated(&self) -> bool {
        self.0 == i64::MAX
    }

    /// Returns the gas value as an integer, rounding the fractional part up.
    #[inline]
    pub const fn round_up(&self) -> i64 {
        milligas_to_gas(self.0, true)
    }

    /// Returns the gas value as an integer, truncating the fractional part.
    #[inline]
    pub const fn round_down(&self) -> i64 {
        milligas_to_gas(self.0, false)
    }

    /// Returns the gas value as milligas, without loss of precision.
    #[inline]
    pub const fn as_milligas(&self) -> i64 {
        self.0
    }
}

impl num_traits::Zero for Gas {
    fn zero() -> Self {
        Gas(0)
    }

    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl Add for Gas {
    type Output = Gas;

    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for Gas {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_add(rhs.0)
    }
}

impl SubAssign for Gas {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_sub(rhs.0)
    }
}

impl Sub for Gas {
    type Output = Gas;

    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Mul<i64> for Gas {
    type Output = Gas;

    #[inline]
    fn mul(self, rhs: i64) -> Self::Output {
        Self(self.0.saturating_mul(rhs))
    }
}

impl Mul<i32> for Gas {
    type Output = Gas;

    #[inline]
    fn mul(self, rhs: i32) -> Self::Output {
        Self(self.0.saturating_mul(rhs.into()))
    }
}

impl Mul<u64> for Gas {
    type Output = Gas;

    #[inline]
    fn mul(self, rhs: u64) -> Self::Output {
        Self(self.0.saturating_mul(rhs.try_into().unwrap_or(i64::MAX)))
    }
}

impl Mul<u32> for Gas {
    type Output = Gas;

    #[inline]
    fn mul(self, rhs: u32) -> Self::Output {
        Self(self.0.saturating_mul(rhs.into()))
    }
}

impl Mul<usize> for Gas {
    type Output = Gas;

    #[inline]
    fn mul(self, rhs: usize) -> Self::Output {
        Self(self.0.saturating_mul(rhs.try_into().unwrap_or(i64::MAX)))
    }
}

pub struct GasTracker {
    gas_limit: Gas,
    gas_used: Cell<Gas>,
    trace: Option<RefCell<Vec<GasCharge>>>,
}

impl GasTracker {
    /// Gas limit and gas used are provided in protocol units (i.e. full units).
    /// They are converted to milligas for internal canonical accounting.
    pub fn new(gas_limit: Gas, gas_used: Gas, enable_tracing: bool) -> Self {
        Self {
            gas_limit,
            gas_used: Cell::new(gas_used),
            trace: enable_tracing.then_some(Default::default()),
        }
    }

    fn charge_gas_inner(&self, to_use: Gas) -> Result<()> {
        // The gas type uses saturating math.
        let gas_used = self.gas_used.get() + to_use;
        if gas_used > self.gas_limit {
            log::trace!("gas limit reached");
            self.gas_used.set(self.gas_limit);
            Err(ExecutionError::OutOfGas)
        } else {
            self.gas_used.set(gas_used);
            Ok(())
        }
    }

    /// Safely consumes gas and returns an out of gas error if there is not sufficient
    /// enough gas remaining for charge.
    pub fn charge_gas(&self, name: &str, to_use: Gas) -> Result<GasTimer> {
        log::trace!("charging gas: {} {}", name, to_use);
        let res = self.charge_gas_inner(to_use);
        if let Some(trace) = &self.trace {
            let mut charge = GasCharge::new(name.to_owned(), to_use, Gas::zero());
            let timer = GasTimer::new(&mut charge.elapsed);
            trace.borrow_mut().push(charge);
            res.map(|_| timer)
        } else {
            res.map(|_| GasTimer::empty())
        }
    }

    /// Applies the specified gas charge, where quantities are supplied in milligas.
    pub fn apply_charge(&self, mut charge: GasCharge) -> Result<GasTimer> {
        let to_use = charge.total();
        log::trace!("charging gas: {} {}", &charge.name, to_use);
        let res = self.charge_gas_inner(to_use);
        if let Some(trace) = &self.trace {
            let timer = GasTimer::new(&mut charge.elapsed);
            trace.borrow_mut().push(charge);
            res.map(|_| timer)
        } else {
            res.map(|_| GasTimer::empty())
        }
    }

    /// Absorbs another GasTracker (usually a nested one) into this one, charging for gas
    /// used and appending all traces.
    pub fn absorb(&self, other: &GasTracker) -> Result<()> {
        if let Some(trace) = &self.trace {
            trace.borrow_mut().extend(other.drain_trace());
        }
        self.charge_gas_inner(other.gas_used())
    }

    /// Make a "child" gas-tracker with a new limit, if and only if the new limit is less than the
    /// available gas.
    pub fn new_child(&self, new_limit: Gas) -> Option<GasTracker> {
        (self.gas_available() > new_limit)
            .then(|| GasTracker::new(new_limit, Gas::zero(), self.trace.is_some()))
    }

    /// Getter for the maximum gas usable by this message.
    pub fn gas_limit(&self) -> Gas {
        self.gas_limit
    }

    /// Getter for gas used.
    pub fn gas_used(&self) -> Gas {
        self.gas_used.get()
    }

    /// Getter for gas available.
    pub fn gas_available(&self) -> Gas {
        self.gas_limit - self.gas_used.get()
    }

    pub fn drain_trace(&self) -> impl Iterator<Item = GasCharge> + '_ {
        self.trace
            .as_ref()
            .map(|v| v.take().into_iter())
            .into_iter()
            .flatten()
    }
}

/// Converts the specified fractional gas units into gas units
#[inline]
pub(crate) const fn milligas_to_gas(milligas: i64, round_up: bool) -> i64 {
    let mut div_result = milligas / MILLIGAS_PRECISION;
    if milligas > 0 && round_up && milligas % MILLIGAS_PRECISION != 0 {
        div_result = div_result.saturating_add(1);
    } else if milligas < 0 && !round_up && milligas % MILLIGAS_PRECISION != 0 {
        div_result = div_result.saturating_sub(1);
    }
    div_result
}

#[cfg(test)]
mod tests {
    use num_traits::Zero;

    use super::*;

    #[test]
    #[allow(clippy::identity_op)]
    fn basic_gas_tracker() -> Result<()> {
        let t = GasTracker::new(Gas::new(20), Gas::new(10), false);
        let _ = t.apply_charge(GasCharge::new("", Gas::new(5), Gas::zero()))?;
        assert_eq!(t.gas_used(), Gas::new(15));
        let _ = t.apply_charge(GasCharge::new("", Gas::new(5), Gas::zero()))?;
        assert_eq!(t.gas_used(), Gas::new(20));
        assert!(t
            .apply_charge(GasCharge::new("", Gas::new(1), Gas::zero()))
            .is_err());
        Ok(())
    }

    #[test]
    fn milligas_to_gas_round() {
        assert_eq!(milligas_to_gas(100, false), 0);
        assert_eq!(milligas_to_gas(100, true), 1);
        assert_eq!(milligas_to_gas(-100, false), -1);
        assert_eq!(milligas_to_gas(-100, true), 0);
    }
}
