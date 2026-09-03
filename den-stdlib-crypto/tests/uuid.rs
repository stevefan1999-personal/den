//! RFC 9562 bit layout for `crypto.randomUUID`.
//!
//! `tests/js/uuid.js` already checks one string against a regex; this walks
//! enough draws that a wrong mask shows up instead of hiding behind a lucky
//! sample.

const SAMPLES: usize = 1000;

#[test]
fn random_uuid_is_version_4_with_the_variant_10_bits() {
    for _ in 0..SAMPLES {
        let uuid = den_stdlib_crypto::random_uuid();
        let groups: Vec<&str> = uuid.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12],
            "{uuid} is not 8-4-4-4-12"
        );
        assert!(
            uuid.chars()
                .all(|character| character == '-' || character.is_ascii_hexdigit()),
            "{uuid} is not hexadecimal"
        );
        let (Some(version), Some(variant)) = (groups.get(2), groups.get(3)) else {
            unreachable!("{uuid} has five groups")
        };
        assert!(version.starts_with('4'), "{uuid} is not version 4");
        assert!(
            variant.starts_with(['8', '9', 'a', 'b']),
            "{uuid} does not carry the 0b10 variant bits"
        );
    }
}
