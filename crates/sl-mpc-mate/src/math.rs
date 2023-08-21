use std::ops::Deref;

use elliptic_curve::ops::Reduce;

use elliptic_curve::{
    bigint::U256, group::GroupEncoding, rand_core::CryptoRngCore,
    CurveArithmetic, Field, Group, NonZeroScalar,
};

use crate::{matrix::matrix_inverse, message::*};

/// A polynomial with coefficients of type `Scalar`.
pub struct Polynomial<C: CurveArithmetic> {
    /// The coefficients of the polynomial.
    pub coeffs: Vec<C::Scalar>,
}

impl<C: CurveArithmetic> Polynomial<C> {
    /// Create a new polynomial with the given coefficients.
    pub fn new(coeffs: Vec<C::Scalar>) -> Self {
        Self { coeffs }
    }

    /// Create a new polynomial with random coefficients.
    pub fn random(rng: &mut impl CryptoRngCore, degree: usize) -> Self {
        Self {
            coeffs: (0..=degree)
                .map(|_| C::Scalar::random(&mut *rng))
                .collect(),
        }
    }

    /// Evaluate the polynomial at 0 (the constant term).
    pub fn get_constant(&self) -> &C::Scalar {
        &self.coeffs[0]
    }

    /// Commit to this polynomial by multiplying each coefficient by the generator.
    pub fn commit(&self) -> GroupPolynomial<C>
    where
        C::ProjectivePoint: GroupEncoding,
    {
        GroupPolynomial::new(
            self.coeffs
                .iter()
                .map(|coeff| {
                    Opaque::from(C::ProjectivePoint::generator() * coeff)
                })
                .collect(),
        )
    }

    /// Computes the n_i derivative of a polynomial with coefficients u_i_k at the point x
    ///
    /// `n`: order of the derivative
    ///
    /// `x`: point at which to compute the derivative.
    /// Arithmetic is done modulo the curve order
    pub fn derivative_at(&self, n: usize, x: &C::Scalar) -> C::Scalar
    where
        C: CurveArithmetic<Uint = U256>,
    {
        (n..self.coeffs.len())
            .map(|i| {
                // TODO build static table of factorials ??
                //      U256::wrapping_mul if const fn
                let num: U256 = factorial_range(i - n, i);
                let scalar_num = C::Scalar::reduce(num);
                let coeff = &self.coeffs[i];
                let result = x.pow_vartime([(i - n) as u64]);

                scalar_num * coeff * result
            })
            .fold(C::Scalar::ZERO, |acc, x| acc + x)
    }
}

/// A polynomial with coefficients of type `ProjectivePoint`.
#[derive(Debug, Clone, PartialEq, Eq, bincode::Encode, bincode::Decode)]
#[bincode(bounds = "C::ProjectivePoint: GroupEncoding")]
pub struct GroupPolynomial<C: CurveArithmetic>
where
    C::ProjectivePoint: GroupEncoding,
{
    pub coeffs: Vec<Opaque<C::ProjectivePoint, GR>>,
}

impl<C: CurveArithmetic> GroupPolynomial<C>
where
    C::ProjectivePoint: GroupEncoding,
{
    /// Create a new polynomial with the given coefficients.
    pub fn new(coeffs: Vec<Opaque<C::ProjectivePoint, GR>>) -> Self {
        Self { coeffs }
    }

    /// Evaluate the polynomial at 0 (the constant term).
    pub fn get_constant(&self) -> &C::ProjectivePoint {
        &self.coeffs[0]
    }

    /// Add another polynomial's coefficients element wise to this one inplace.
    /// If the other polynomial has more coefficients than this one, the extra
    /// coefficients are ignored.
    pub fn add_mut(&mut self, other: &Self) {
        self.coeffs
            .iter_mut()
            .zip(&other.coeffs)
            .for_each(|(a, b)| {
                a.0 += b.0; // TODO implement AddAssign for Opaque
            });
    }

    /// Get the coeffs of the polynomial derivative
    pub fn derivative_coeffs(&self, n: usize) -> Vec<C::ProjectivePoint>
    where
        C: CurveArithmetic<Uint = U256>,
    {
        let (_, sub_v) = self.coeffs.split_at(n);

        sub_v
            .iter()
            .enumerate()
            .map(|(position, u_i)| {
                *u_i * C::Scalar::reduce(factorial_range(
                    position,
                    position + n,
                ))
            })
            .collect()
    }

    pub fn points(
        &self,
    ) -> impl Iterator<Item = &'_ <C as CurveArithmetic>::ProjectivePoint>
    {
        self.coeffs.iter().map(|p| &p.0)
    }

    pub fn get(&self, idx: usize) -> Option<&C::ProjectivePoint> {
        self.coeffs.get(idx).map(|p| &p.0)
    }
}

impl<C: CurveArithmetic> Deref for Polynomial<C> {
    type Target = [C::Scalar];

    fn deref(&self) -> &Self::Target {
        &self.coeffs
    }
}

// impl<C> Deref for GroupPolynomial<C>
// where
//     C: CurveArithmetic,
//     C::ProjectivePoint: GroupEncoding,
// {
//     type Target = [C::ProjectivePoint];

//     fn deref(&self) -> &Self::Target {
//         &self.coeffs
//     }
// }

/// Computes the factorial of a number, n <= 57 (the largest factorial that fits in 256 bits)
/// This is okay for our purposes because we expect threshold values to be less than 57
/// (i.e. we don't expect to have more than 57 participants)
pub fn factorial(n: usize) -> U256 {
    if n > 57 {
        panic!("Factorial of {} is too large to fit in 256 bits", n);
    }

    (1..=n).fold(U256::from(1_u64), |acc, x| {
        acc.wrapping_mul(&U256::from(x as u64))
    })
}

/// Computes the factorial of a range of numbers (start, end], where end <= 57
pub fn factorial_range(start: usize, end: usize) -> U256 {
    // TODO: Confirm max possible sizes for start and end
    if end > 57 {
        panic!("Factorial of {} is too large to fit in 256 bits", end);
    }

    (start + 1..=end).fold(U256::from_u64(1_u64), |acc, x| {
        acc.wrapping_mul(&U256::from_u64(x as u64))
    })
}

/// Feldman verification
pub fn feldman_verify<C: CurveArithmetic>(
    u_i_k: &[C::ProjectivePoint],
    x_i: &NonZeroScalar<C>,
    f_i_value: &C::Scalar,
    g: &C::ProjectivePoint,
) -> Option<bool> {
    if u_i_k.is_empty() {
        return None;
    }

    let mut point = C::ProjectivePoint::identity();

    for (i, coeff) in u_i_k.iter().enumerate() {
        // x_i^i mod p
        let val = x_i.pow([i as u64]);

        // x_i^i * coeff mod p
        point += *coeff * val;
    }

    let expected_point = *g * f_i_value;

    Some(point == expected_point)
}

/// Get the multipliers for the coefficients of the polynomial,
/// given the x_i (point of evaluation),
/// `n_i` (order of derivative)
/// `n` (degree of polynomial - 1)
/// `p` prime order of field
pub fn polynomial_coeff_multipliers<C: CurveArithmetic>(
    x_i: &NonZeroScalar<C>,
    n_i: usize,
    n: usize,
) -> Vec<C::Scalar>
where
    C: CurveArithmetic<Uint = U256>,
{
    let mut v = vec![C::Scalar::ZERO; n];

    v.iter_mut().enumerate().skip(n_i).for_each(|(idx, vi)| {
        let num = C::Scalar::reduce(factorial_range(idx - n_i, idx));
        let exponent = [(idx - n_i) as u64];
        let result = x_i.pow_vartime(exponent);
        *vi = num * result;
    });

    v
}

/// Get the birkhoff coefficients
pub fn birkhoff_coeffs<C>(
    params: &[(NonZeroScalar<C>, usize)],
) -> Vec<C::Scalar>
where
    C: CurveArithmetic<Uint = U256>,
{
    let n = params.len();

    let matrix: Vec<Vec<C::Scalar>> = params
        .iter()
        .map(|(x_i, n_i)| polynomial_coeff_multipliers(x_i, *n_i, n))
        .collect();

    let mut matrix_inv = matrix_inverse::<C>(matrix, n);

    matrix_inv.swap_remove(0)
}
