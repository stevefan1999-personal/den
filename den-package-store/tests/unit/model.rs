use super::{BlobDigest, DependencyKind, PackageKey, RegistryId, VersionId};
use crate::PackageStoreError;

#[test]
fn blob_digest_round_trips_and_rejects_the_wrong_length() {
    let digest = BlobDigest::for_bytes(b"den");
    assert_eq!(digest.as_bytes().len(), BlobDigest::LEN);
    assert_eq!(
        BlobDigest::from_database(digest.as_bytes().to_vec()).expect("same length"),
        digest
    );
    assert!(matches!(
      BlobDigest::from_database(vec![0; 8]),
      Err(PackageStoreError::InvalidSnapshot(message)) if message.contains("8 bytes")
    ));
    let rendered = format!("{digest}");
    assert_eq!(rendered.len(), BlobDigest::LEN * 2);
    assert_eq!(format!("{digest:?}"), rendered);
}

#[test]
fn dependency_kind_round_trips_through_the_database_label() {
    for kind in [
        DependencyKind::Normal,
        DependencyKind::Optional,
        DependencyKind::Peer,
        DependencyKind::PeerOptional,
    ] {
        let label = kind.as_str();
        assert_eq!(DependencyKind::from_database(label).expect(label), kind);
    }
    assert!(matches!(
      DependencyKind::from_database("dev"),
      Err(PackageStoreError::InvalidSnapshot(message)) if message.contains("dev")
    ));
}

#[test]
fn identifiers_and_empty_snapshots_display_their_keys() {
    assert_eq!(format!("{}", RegistryId(9)), "9");
    let _ = VersionId(3);
    let key = PackageKey {
        registry_id: RegistryId(2),
        name:        "pkg".into(),
    };
    assert_eq!(format!("{key}"), "2:pkg");
}
