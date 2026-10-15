//! The size check applied to a file the browser hands over, before any of it is read (#1333).

/// Whether a browser `File` of `size` bytes (`File.size`, a JS number) may be read under a budget
/// of `max` bytes. The error names the file and the limit.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn check_size(name: &str, size: f64, max: u64) -> Result<(), String> {
    if !size.is_finite() || size < 0.0 {
        return Err(format!("{name}: the browser reported no usable size"));
    }
    if size > max as f64 {
        let limit = if max >= 1 << 20 { format!("{} MB", max >> 20) } else { format!("{max} bytes") };
        return Err(format!("{name}: larger than the limit of {limit}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_over_budget_is_refused_by_name() {
        let err = check_size("big.docx", 101.0, 100).unwrap_err();
        assert!(err.contains("big.docx") && err.contains("100"), "{err}");
    }

    #[test]
    fn a_file_within_budget_is_allowed() {
        assert!(check_size("a.docx", 100.0, 100).is_ok());
        assert!(check_size("empty.txt", 0.0, 100).is_ok());
    }

    #[test]
    fn a_nonsense_size_is_refused() {
        assert!(check_size("x", f64::NAN, 100).is_err());
        assert!(check_size("x", -1.0, 100).is_err());
        assert!(check_size("x", f64::INFINITY, 100).is_err());
    }
}
