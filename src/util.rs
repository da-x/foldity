/// Return the number of items to assign to a slot `idx` out of `0..n - 1`, if we want to divide
/// `a` items as equal as possible to `n` slots.
pub fn most_equal_divide(a: u64, n: u64, idx: u64) -> u64 {
    let mut d = a / n;
    if idx < a % n {
        d += 1;
    }
    d
}
