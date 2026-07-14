//! Core library for the Gleon visual regression testing CLI.

pub mod cli;

/// Adds two numbers together.
///
/// # Examples
/// ```
/// use gleon_core::add;
/// assert_eq!(add(2, 2), 4);
/// ```
pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
