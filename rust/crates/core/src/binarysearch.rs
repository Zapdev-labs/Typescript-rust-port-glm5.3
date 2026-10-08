// Ported from tsc/internal/core/binarysearch.go @ ec47d33c23e464a17cdf2475632cba629bee8763

// BinarySearchUniqueFunc works like slices.BinarySearchFunc, but avoids extra
// invocations of the comparison function by assuming that only one element
// in the slice could match the target. Also, unlike slices.BinarySearchFunc,
// the comparison function is passed the current index of the element being
// compared, instead of the target element.
pub fn binary_search_unique_func<T>(
    x: &[T],
    mut cmp: impl FnMut(usize, &T) -> i32,
) -> (usize, bool) {
    let n = x.len();
    if n == 0 {
        return (0, false);
    }
    let mut low: usize = 0;
    let mut high: usize = n - 1;
    while low <= high {
        let middle = low + ((high - low) >> 1);
        let value = cmp(middle, &x[middle]);
        if value < 0 {
            low = middle + 1;
        } else if value > 0 {
            if middle == 0 {
                // PORT: Go's unsigned underflow (`high = middle - 1`) makes
                // the loop condition false via huge values; Rust usize would
                // panic, so exit explicitly.
                return (low, false);
            }
            high = middle - 1;
        } else {
            return (middle, true);
        }
    }
    (low, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_existing_and_missing() {
        let xs = [10, 20, 30, 40, 50];
        // cmp(element, target): mirror slices.BinarySearchFunc's cmp(element, target) sign.
        let find = |target: i32| {
            binary_search_unique_func(&xs, |_i, e| *e - target)
        };
        assert_eq!(find(30), (2, true));
        assert_eq!(find(10), (0, true));
        assert_eq!(find(50), (4, true));
        assert_eq!(find(35), (3, false));
        assert_eq!(find(60), (5, false));
        assert_eq!(find(-1), (0, false));
        let empty: [i32; 0] = [];
        assert_eq!(
            binary_search_unique_func(&empty, |_i, e: &i32| -e),
            (0, false)
        );
    }
}
