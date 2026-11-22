/// Component values of a passive treble/middle/bass tone stack, in ohms and farads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FmvStack {
    /// Treble pot.
    pub r1: f64,
    /// Bass pot.
    pub r2: f64,
    /// Mid pot.
    pub r3: f64,
    /// Slope resistor.
    pub r4: f64,
    /// Treble capacitor.
    pub c1: f64,
    /// Bass capacitor.
    pub c2: f64,
    /// Mid capacitor.
    pub c3: f64,
}

/// Third-order transfer function as (numerator, denominator), lowest power first.
pub type Coefficients = ([f64; 4], [f64; 4]);

/// Analog transfer function of `stack` for pot positions in 0..=1.
// Transfer function from D. T. Yeh, J. O. Smith, "Discretization of the '59 Fender Bassman Tone Stack", DAFx 2006.
pub fn analog_coefficients(stack: &FmvStack, treble: f64, middle: f64, bass: f64) -> Coefficients {
    let FmvStack {
        r1,
        r2,
        r3,
        r4,
        c1,
        c2,
        c3,
    } = *stack;
    let (t, m, l) = (treble, middle, bass);
    let b1 = t * c1 * r1 + m * c3 * r3 + l * (c1 * r2 + c2 * r2) + (c1 * r3 + c2 * r3);
    let b2 = t * (c1 * c2 * r1 * r4 + c1 * c3 * r1 * r4)
        - m * m * (c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + m * (c1 * c3 * r1 * r3 + c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + l * (c1 * c2 * r1 * r2 + c1 * c2 * r2 * r4 + c1 * c3 * r2 * r4)
        + l * m * (c1 * c3 * r2 * r3 + c2 * c3 * r2 * r3)
        + (c1 * c2 * r1 * r3 + c1 * c2 * r3 * r4 + c1 * c3 * r3 * r4);
    let b3 = l * m * (c1 * c2 * c3 * r1 * r2 * r3 + c1 * c2 * c3 * r2 * r3 * r4)
        - m * m * (c1 * c2 * c3 * r1 * r3 * r3 + c1 * c2 * c3 * r3 * r3 * r4)
        + m * (c1 * c2 * c3 * r1 * r3 * r3 + c1 * c2 * c3 * r3 * r3 * r4)
        + t * c1 * c2 * c3 * r1 * r3 * r4
        - t * m * c1 * c2 * c3 * r1 * r3 * r4
        + t * l * c1 * c2 * c3 * r1 * r2 * r4;
    let a1 =
        (c1 * r1 + c1 * r3 + c2 * r3 + c2 * r4 + c3 * r4) + m * c3 * r3 + l * (c1 * r2 + c2 * r2);
    let a2 = m * (c1 * c3 * r1 * r3 - c2 * c3 * r3 * r4 + c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + l * m * (c1 * c3 * r2 * r3 + c2 * c3 * r2 * r3)
        - m * m * (c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
        + l * (c1 * c2 * r2 * r4 + c1 * c2 * r1 * r2 + c1 * c3 * r2 * r4 + c2 * c3 * r2 * r4)
        + (c1 * c2 * r1 * r4
            + c1 * c3 * r1 * r4
            + c1 * c2 * r3 * r4
            + c1 * c2 * r1 * r3
            + c1 * c3 * r3 * r4
            + c2 * c3 * r3 * r4);
    let a3 = l * m * (c1 * c2 * c3 * r1 * r2 * r3 + c1 * c2 * c3 * r2 * r3 * r4)
        - m * m * (c1 * c2 * c3 * r1 * r3 * r3 + c1 * c2 * c3 * r3 * r3 * r4)
        + m * (c1 * c2 * c3 * r3 * r3 * r4 + c1 * c2 * c3 * r1 * r3 * r3
            - c1 * c2 * c3 * r1 * r3 * r4)
        + l * c1 * c2 * c3 * r1 * r2 * r4
        + c1 * c2 * c3 * r1 * r3 * r4;
    ([0.0, b1, b2, b3], [1.0, a1, a2, a3])
}

/// Bilinear transform of analog coefficients with `c` = 2 * sample rate, normalised so `a[0]` is 1.
pub fn bilinear((b, a): Coefficients, c: f64) -> Coefficients {
    let discretise = |k: [f64; 4]| {
        let [k0, k1, k2, k3] = [k[0], k[1] * c, k[2] * c * c, k[3] * c * c * c];
        [
            k0 + k1 + k2 + k3,
            3.0 * k0 + k1 - k2 - 3.0 * k3,
            3.0 * k0 - k1 - k2 + 3.0 * k3,
            k0 - k1 + k2 - k3,
        ]
    };
    let (b, a) = (discretise(b), discretise(a));
    (b.map(|k| k / a[0]), a.map(|k| k / a[0]))
}

/// One sample of the discrete filter in transposed direct form II.
#[allow(
    clippy::many_single_char_names,
    reason = "b, a, s, x, y are the difference equation's textbook names"
)]
pub fn step(coefficients: &Coefficients, state: &mut [f64; 3], x: f64) -> f64 {
    let (b, a) = coefficients;
    let s = state;
    let y = b[0] * x + s[0];
    s[0] = b[1] * x - a[1] * y + s[1];
    s[1] = b[2] * x - a[2] * y + s[2];
    s[2] = b[3] * x - a[3] * y;
    y
}
