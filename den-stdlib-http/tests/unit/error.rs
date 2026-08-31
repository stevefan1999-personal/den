use super::{HttpError, HttpErrorKind};

#[test]
fn error_kinds_render_their_stable_names() {
    assert_eq!(HttpErrorKind::Aborted.as_str(), "Aborted");
    assert_eq!(HttpErrorKind::AddrInUse.as_str(), "AddrInUse");
    assert_eq!(HttpErrorKind::Bind.as_str(), "Bind");
    let error = HttpError::from_kind(HttpErrorKind::Bind, "address in use");
    assert_eq!(error.name(), "HttpError");
    assert_eq!(error.kind(), "Bind");
    assert_eq!(error.message(), "address in use");
}
