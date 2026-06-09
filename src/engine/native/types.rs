use std::collections::HashMap;

/// A dimensional quantity: a numeric factor paired with primitive base-unit
/// exponents.  Only "primitive" units (those defined with `!` in the
/// definitions file) appear as dimension keys.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UnitValue {
    pub factor: f64,
    /// Primitive unit name → integer exponent.
    pub dimensions: HashMap<String, i32>,
}

impl UnitValue {
    pub fn one() -> Self {
        Self {
            factor: 1.0,
            dimensions: HashMap::new(),
        }
    }

    pub fn from_factor(f: f64) -> Self {
        Self {
            factor: f,
            dimensions: HashMap::new(),
        }
    }

    pub fn primitive(name: impl Into<String>) -> Self {
        let mut dimensions = HashMap::new();
        dimensions.insert(name.into(), 1);
        Self {
            factor: 1.0,
            dimensions,
        }
    }

    pub fn is_dimensionless(&self) -> bool {
        self.dimensions.values().all(|&e| e == 0)
    }

    fn normalize(&mut self) {
        self.dimensions.retain(|_, v| *v != 0);
    }

    pub fn multiply_assign(&mut self, rhs: &Self) {
        self.factor *= rhs.factor;
        for (unit, &exp) in &rhs.dimensions {
            *self.dimensions.entry(unit.clone()).or_insert(0) += exp;
        }
        self.normalize();
    }

    pub fn divide_assign(&mut self, rhs: &Self) {
        self.factor /= rhs.factor;
        for (unit, &exp) in &rhs.dimensions {
            *self.dimensions.entry(unit.clone()).or_insert(0) -= exp;
        }
        self.normalize();
    }

    /// Returns `true` iff the units are conformable (same dimensions after
    /// normalisation) and the addition succeeds.
    pub fn add_assign(&mut self, rhs: &Self) -> bool {
        let self_dims: HashMap<&String, i32> = self
            .dimensions
            .iter()
            .filter(|(_, v)| **v != 0)
            .map(|(k, v)| (k, *v))
            .collect();
        let rhs_dims: HashMap<&String, i32> = rhs
            .dimensions
            .iter()
            .filter(|(_, v)| **v != 0)
            .map(|(k, v)| (k, *v))
            .collect();
        if self_dims != rhs_dims {
            return false;
        }
        self.factor += rhs.factor;
        true
    }

    pub fn invert(&mut self) {
        self.factor = 1.0 / self.factor;
        for exp in self.dimensions.values_mut() {
            *exp = exp.wrapping_neg();
        }
    }

    pub fn pow_assign(&mut self, n: i32) {
        self.factor = self.factor.powi(n);
        for exp in self.dimensions.values_mut() {
            *exp *= n;
        }
        self.normalize();
    }

    /// Returns `false` if any dimension exponent is not divisible by `n`,
    /// or if the factor is negative (matching C engine behavior: roots of
    /// negative numbers always fail, even odd roots).
    pub fn root_assign(&mut self, n: i32) -> bool {
        if n <= 0 {
            return false;
        }
        if self.factor < 0.0 {
            return false;
        }
        for exp in self.dimensions.values() {
            if exp % n != 0 {
                return false;
            }
        }
        self.factor = self.factor.powf(1.0 / n as f64);
        for exp in self.dimensions.values_mut() {
            *exp /= n;
        }
        self.normalize();
        true
    }

    /// Human-readable base-units string, e.g. `"kg m / s s"`.
    pub fn base_units_string(&self) -> String {
        let mut numerators: Vec<&str> = Vec::new();
        let mut denominators: Vec<&str> = Vec::new();

        let mut sorted: Vec<(&String, &i32)> = self.dimensions.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());

        for (unit, exp) in &sorted {
            let exp = **exp;
            match exp.cmp(&0) {
                std::cmp::Ordering::Greater => {
                    for _ in 0..exp {
                        numerators.push(unit.as_str());
                    }
                }
                std::cmp::Ordering::Less => {
                    for _ in 0..exp.unsigned_abs() {
                        denominators.push(unit.as_str());
                    }
                }
                std::cmp::Ordering::Equal => {}
            }
        }

        if numerators.is_empty() && denominators.is_empty() {
            return String::new();
        }
        if denominators.is_empty() {
            return numerators.join(" ");
        }
        let num_part = if numerators.is_empty() {
            "1".to_owned()
        } else {
            numerators.join(" ")
        };
        format!("{num_part} / {}", denominators.join(" "))
    }
}

impl Default for UnitValue {
    fn default() -> Self {
        Self::one()
    }
}

/// Converts a floating-point value to a rational approximation `p/q` using a
/// continued-fraction expansion (up to 20 terms), replicating `float2rat` from
/// the GNU units C source.
///
/// Returns `Some((p, q))` when `q < 100` and the approximation error is within
/// [`f64::EPSILON`].  Returns `None` when the value cannot be approximated by a
/// small-denominator rational (irrational or too large).
pub(crate) fn float2rat(y: f64) -> Option<(i32, i32)> {
    let mut coef = [0i32; 20];
    let mut x = y;
    let mut termcount = 0usize;

    loop {
        let floor_val = x.floor();
        if !floor_val.is_finite() || floor_val > i32::MAX as f64 || floor_val < i32::MIN as f64 {
            return None;
        }
        coef[termcount] = floor_val as i32;
        let fracpart = x - floor_val;
        if fracpart < 0.001 || termcount == 19 {
            break;
        }
        x = 1.0 / fracpart;
        termcount += 1;
    }

    let mut p: i32 = 0;
    let mut q: i32 = 1;
    for i in (1..=termcount).rev() {
        let saveq = q;
        q = coef[i].saturating_mul(q).saturating_add(p);
        p = saveq;
    }
    p = p.saturating_add(q.saturating_mul(coef[0]));

    if q < 100 && (p as f64 / q as f64 - y).abs() < f64::EPSILON {
        Some((p, q))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn one_is_dimensionless() {
        let v = UnitValue::one();

        assert_eq!(v.factor, 1.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn from_factor_preserves_value() {
        let v = UnitValue::from_factor(42.0);

        assert_eq!(v.factor, 42.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn primitive_has_dimension() {
        let v = UnitValue::primitive("m");

        assert_eq!(v.factor, 1.0);
        assert_eq!(v.dimensions.get("m"), Some(&1));
        assert!(!v.is_dimensionless());
    }

    #[test]
    fn multiply_assign_factors() {
        let mut lhs = UnitValue::from_factor(3.0);
        let rhs = UnitValue::from_factor(4.0);

        lhs.multiply_assign(&rhs);

        assert_eq!(lhs.factor, 12.0);
    }

    #[test]
    fn multiply_assign_dimensions() {
        let mut lhs = UnitValue::primitive("m");
        let rhs = UnitValue::primitive("kg");

        lhs.multiply_assign(&rhs);

        assert_eq!(lhs.dimensions.get("m"), Some(&1));
        assert_eq!(lhs.dimensions.get("kg"), Some(&1));
    }

    #[test]
    fn divide_assign_factors() {
        let mut lhs = UnitValue::from_factor(10.0);
        let rhs = UnitValue::from_factor(2.0);

        lhs.divide_assign(&rhs);

        assert!((lhs.factor - 5.0).abs() < 1e-12);
    }

    #[test]
    fn divide_assign_dimensions() {
        let mut lhs = UnitValue::primitive("m");
        let rhs = UnitValue::primitive("s");

        lhs.divide_assign(&rhs);

        assert_eq!(lhs.dimensions.get("m"), Some(&1));
        assert_eq!(lhs.dimensions.get("s"), Some(&-1));
    }

    #[test]
    fn divide_assign_same_unit_cancels() {
        let mut lhs = UnitValue::primitive("m");
        let rhs = UnitValue::primitive("m");

        lhs.divide_assign(&rhs);

        assert!(lhs.is_dimensionless());
        assert!(!lhs.dimensions.contains_key("m"));
    }

    #[test]
    fn add_assign_same_dimensions() {
        let mut lhs = UnitValue::primitive("m");
        let mut rhs = UnitValue::primitive("m");
        lhs.factor = 3.0;
        rhs.factor = 7.0;

        let ok = lhs.add_assign(&rhs);

        assert!(ok);
        assert_eq!(lhs.factor, 10.0);
        assert_eq!(lhs.dimensions.get("m"), Some(&1));
    }

    #[test]
    fn add_assign_different_fails() {
        let mut lhs = UnitValue::primitive("m");
        let rhs = UnitValue::primitive("kg");

        let ok = lhs.add_assign(&rhs);

        assert!(!ok);
    }

    #[test]
    fn invert_factor() {
        let mut v = UnitValue::from_factor(4.0);

        v.invert();

        assert!((v.factor - 0.25).abs() < 1e-12);
    }

    #[test]
    fn invert_dimensions() {
        let mut v = UnitValue::primitive("m");

        v.invert();

        assert_eq!(v.dimensions.get("m"), Some(&-1));
    }

    #[rstest]
    #[case::square(2, 9.0, 2)]
    #[case::cube(3, 27.0, 3)]
    #[case::zero_pow(0, 1.0, 0)]
    fn pow_assign_factor_and_dims(
        #[case] n: i32,
        #[case] expected_factor: f64,
        #[case] expected_exp: i32,
    ) {
        let mut v = UnitValue::primitive("m");
        v.factor = 3.0;

        v.pow_assign(n);

        assert!((v.factor - expected_factor).abs() < 1e-9);
        if expected_exp != 0 {
            assert_eq!(v.dimensions.get("m"), Some(&expected_exp));
        } else {
            assert!(!v.dimensions.contains_key("m"));
        }
    }

    #[test]
    fn root_assign_success() {
        let mut v = UnitValue::primitive("m");
        v.pow_assign(4);
        v.factor = 16.0;

        let ok = v.root_assign(2);

        assert!(ok);
        assert!((v.factor - 4.0).abs() < 1e-12);
        assert_eq!(v.dimensions.get("m"), Some(&2));
    }

    #[test]
    fn root_assign_non_divisible_fails() {
        let mut v = UnitValue::primitive("m");

        let ok = v.root_assign(2);

        assert!(!ok);
        assert_eq!(v.dimensions.get("m"), Some(&1));
    }

    #[test]
    fn base_units_string_empty() {
        let v = UnitValue::from_factor(5.0);

        let s = v.base_units_string();

        assert_eq!(s, "");
    }

    #[test]
    fn base_units_string_numerator_only() {
        let mut v = UnitValue::primitive("kg");
        let m = UnitValue::primitive("m");
        v.multiply_assign(&m);

        let s = v.base_units_string();

        assert!(s.contains("kg"), "expected 'kg' in '{s}'");
        assert!(s.contains("m"), "expected 'm' in '{s}'");
        assert!(!s.contains('/'), "unexpected '/' in '{s}'");
    }

    #[test]
    fn base_units_string_both() {
        let mut v = UnitValue::primitive("m");
        let s2 = UnitValue::primitive("s");
        v.divide_assign(&s2);

        let s = v.base_units_string();

        assert!(s.contains("m"), "expected 'm' in '{s}'");
        assert!(s.contains(" / "), "expected ' / ' in '{s}'");
        assert!(s.contains('s'), "expected 's' in '{s}'");
    }
}
