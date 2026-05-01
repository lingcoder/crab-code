/// Generates a new ULID (Universally Unique Lexicographically Sortable Identifier).
///
/// Uses a thread-local monotonic generator to guarantee lexicographic ordering
/// even when multiple ULIDs are created within the same millisecond.
#[must_use]
pub fn new_ulid() -> String {
    use std::cell::RefCell;
    use ulid::Generator;

    thread_local! {
        static GEN: RefCell<Generator> = const { RefCell::new(Generator::new()) };
    }

    GEN.with(|g| {
        g.borrow_mut()
            .generate()
            .expect("ULID generation should not fail")
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_is_26_chars() {
        let id = new_ulid();
        assert_eq!(id.len(), 26);
    }

    #[test]
    fn ulid_is_uppercase_crockford_base32() {
        let id = new_ulid();
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn ulid_uniqueness() {
        let id1 = new_ulid();
        let id2 = new_ulid();
        assert_ne!(id1, id2);
    }

    #[test]
    fn ulid_monotonic_ordering() {
        // ULIDs generated in sequence should be lexicographically ordered
        let id1 = new_ulid();
        let id2 = new_ulid();
        assert!(id1 <= id2);
    }

    #[test]
    fn ulid_roundtrip_parse() {
        let id = new_ulid();
        let parsed = ulid::Ulid::from_string(&id);
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().to_string(), id);
    }

    #[test]
    fn ulid_monotonic_ordering_batch() {
        // Generate 100 ULIDs in rapid succession; each must be >= previous
        let ids: Vec<String> = (0..100).map(|_| new_ulid()).collect();
        for pair in ids.windows(2) {
            assert!(pair[0] <= pair[1], "{} should be <= {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn ulid_all_unique_in_batch() {
        let ids: Vec<String> = (0..100).map(|_| new_ulid()).collect();
        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len());
    }
}
